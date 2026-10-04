//! Long-poll wake-ups, silent startup priming and `endTime` paging.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{
    JPEG,
    feed::{camera, record},
};
use crate::{
    alarms::{AlarmFeed, AlarmSource, fetch},
    config::CameraConfig,
    error::BridgeError,
    metrics::Metrics,
};

const BASE_MS: i64 = 1_700_000_000_000;

#[derive(Default)]
struct Fake {
    summary: Value,
    /// Pages keyed by the `endTime` cursor.
    pages: BTreeMap<String, Value>,
    calls: Vec<(String, String)>,
}

struct FakeSource(Mutex<Fake>);

#[async_trait]
impl AlarmSource for FakeSource {
    async fn alarm_summary(&self) -> Result<Value, BridgeError> {
        Ok(self.0.lock().await.summary.clone())
    }

    async fn alarm_list(&self, _: &str, date: &str, end: &str) -> Result<Value, BridgeError> {
        let mut fake = self.0.lock().await;
        fake.calls.push((date.to_owned(), end.to_owned()));
        Ok(fake
            .pages
            .get(end)
            .cloned()
            .unwrap_or_else(|| json!({"hasNext":false,"message":[]})))
    }

    async fn alarm_picture(&self, url: &str, _: usize) -> Result<Vec<u8>, BridgeError> {
        if url.ends_with("/ok") {
            Ok(JPEG.to_vec())
        } else {
            Err(BridgeError::Upstream("missing".into()))
        }
    }

    async fn picture_keys(&self, _: &CameraConfig) -> Result<Vec<String>, BridgeError> {
        Err(BridgeError::Upstream("unused".into()))
    }
}

fn message(index: i64, picture: &str) -> Value {
    json!({"msgId": format!("m{index}"), "deviceSerial": "CAM1", "channel": 1,
        "time": BASE_MS + index * 1000, "title": format!("Alarm {index}"),
        "pic": format!("https://pictures.example.invalid/{picture}"), "picCrypt": 0,
        "ext": {"alarmType": 10_002}})
}

fn summary(top: i64) -> Value {
    json!({"summaries":[{"deviceSerial":"CAM1","total":top,"topMessage":{"msgId":format!("m{top}")}}]})
}

fn page(indices: &[i64], has_next: bool) -> Value {
    let messages: Vec<Value> = indices.iter().map(|index| message(*index, "ok")).collect();
    json!({"meta":{"code":200},"hasNext":has_next,"message":messages})
}

fn feed(directory: &std::path::Path) -> Arc<AlarmFeed> {
    let cameras = BTreeMap::from([("front".to_owned(), camera("CAM1"))]);
    AlarmFeed::load(directory, &cameras, Metrics::default())
}

async fn eventually(feed: &AlarmFeed, head: u64) {
    for _ in 0..200 {
        if feed
            .batch("front", None, Duration::ZERO)
            .await
            .map_or(0, |b| b.next_sequence)
            >= head
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("alarm history did not reach {head}");
}

#[tokio::test]
async fn long_poll_returns_recent_history_and_wakes_on_publish() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let feed = feed(directory.path());
    let empty = feed.batch("front", Some(0), Duration::ZERO).await;
    assert!(empty.is_ok_and(|batch| batch.events.is_empty() && batch.next_sequence == 0));
    let waiter = tokio::spawn({
        let feed = Arc::clone(&feed);
        async move { feed.batch("front", Some(0), Duration::from_secs(5)).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let batch = vec![(record("x1", 5), Some(JPEG.to_vec()))];
    let outcome = feed.publish("front", batch, true).await;
    assert!(outcome.is_ok_and(|value| value.inserted == 1));
    let woke = waiter
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((woke.next_sequence, woke.events.len()), (1, 1));
    assert!(woke.events[0].has_picture);
    assert_eq!(woke.generation, feed.generation());
    let recent = feed.batch("front", None, Duration::from_secs(5)).await;
    assert!(recent.is_ok_and(|batch| batch.events.len() == 1 && batch.next_sequence == 1));
    assert!(feed.batch("missing", None, Duration::ZERO).await.is_err());
    assert!(matches!(feed.picture("front", "x1").await, Ok(Some(data)) if data == JPEG));
    assert!(matches!(feed.picture("front", "../x1").await, Ok(None)));
}

#[tokio::test]
async fn startup_priming_keeps_history_without_replaying_it() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let feed = feed(directory.path());
    let state = Fake {
        summary: summary(2),
        pages: BTreeMap::from([(String::new(), page(&[2, 1], false))]),
        ..Fake::default()
    };
    let source = Arc::new(FakeSource(Mutex::new(state)));
    feed.spawn(Arc::clone(&source) as _, Duration::from_millis(20));
    eventually(&feed, 2).await;
    let cursor = feed.batch("front", Some(0), Duration::ZERO).await;
    assert!(cursor.is_ok_and(|batch| batch.events.is_empty() && batch.next_sequence == 2));
    {
        let mut fake = source.0.lock().await;
        fake.summary = summary(4);
        fake.pages.insert(String::new(), page(&[4, 3, 2], true));
    }
    eventually(&feed, 4).await;
    let batch = feed
        .batch("front", Some(2), Duration::ZERO)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let ids: Vec<_> = batch.events.iter().map(|event| event.id.as_str()).collect();
    assert_eq!(ids, ["m3", "m4"]);
    assert!(batch.events.iter().all(|event| event.has_picture));
    assert_eq!(batch.events[1].title, "Alarm 4");
    feed.close().await;
    let metrics = feed.metrics.render().await;
    assert!(metrics.contains("ezviz_bridge_alarms_received_total{camera=\"front\"} 2"));
    assert!(metrics.contains("ezviz_bridge_alarm_pictures_stored_total{camera=\"front\"} 4"));
    let calls = source.0.lock().await.calls.clone();
    assert!(
        calls
            .iter()
            .all(|(date, end)| date.len() == 8 && end.is_empty())
    );
}

#[tokio::test]
async fn paging_follows_end_time_and_respects_the_page_budget() {
    let first_end = (BASE_MS + 5000).to_string();
    let second_end = (BASE_MS + 3000).to_string();
    let state = Fake {
        pages: BTreeMap::from([
            (String::new(), page(&[6, 5], true)),
            (first_end.clone(), page(&[4, 3], true)),
            (second_end.clone(), page(&[2, 1], true)),
        ]),
        ..Fake::default()
    };
    let source = FakeSource(Mutex::new(state));
    let known = |id: &str| id == "m3";
    let result = fetch::fetch_unknown(&source, "CAM1", Some(3600), None, &known, 3).await;
    let result = result.unwrap_or_else(|error| panic!("{error}"));
    let ids: Vec<_> = result
        .messages
        .iter()
        .map(|m| m.record.id.as_str())
        .collect();
    assert_eq!(ids, ["m4", "m5", "m6"]);
    assert!(!result.truncated);
    let nothing_known = |_: &str| false;
    let result = fetch::fetch_unknown(&source, "CAM1", None, None, &nothing_known, 3).await;
    let result = result.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.messages.len(), 6);
    assert!(result.truncated);
    let ends: Vec<_> = source
        .0
        .lock()
        .await
        .calls
        .iter()
        .map(|c| c.1.clone())
        .collect();
    assert_eq!(ends, ["", &first_end, "", &first_end, &second_end]);
}

#[test]
fn camera_local_day_uses_the_learned_offset() {
    let now = chrono::DateTime::from_timestamp(1_699_999_000, 0).unwrap_or_default();
    let day = fetch::local_day(Some(3600), now);
    assert_eq!(fetch::api_date(day), "20231114");
    assert_eq!(
        fetch::api_date(fetch::local_day(Some(10_800), now)),
        "20231115"
    );
    let midnight = fetch::local_midnight(day, Some(3600), now);
    assert_eq!(midnight, 1_699_916_400);
}
