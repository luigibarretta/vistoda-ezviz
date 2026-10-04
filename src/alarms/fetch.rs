//! Bounded `list/v2` paging for one device serial.
//!
//! The `date` parameter is the camera-local day. The local offset is learned
//! from the device's own messages (`timeStr` local versus `time` epoch);
//! until one is seen, the process time zone (`TZ`) is used, falling back to
//! UTC. Paging uses the last item's `time` (epoch milliseconds) as `endTime`
//! and stops at a known alarm, `hasNext=false`, a page without progress, or
//! the page budget. Near midnight the previous local day is also consulted
//! when today's pages end before reaching any known alarm.

use std::collections::BTreeSet;

use chrono::{DateTime, Days, Local, NaiveDate, Utc};

use super::{AlarmSource, model::ParsedMessage, parse_list};
use crate::error::BridgeError;

/// Pages fetched for one changed device per poll cycle.
pub const LIVE_PAGES: usize = 3;
/// Pages fetched to prime a device at startup.
pub const PRIME_PAGES: usize = 1;

pub struct FetchResult {
    /// Unknown messages, oldest first.
    pub messages: Vec<ParsedMessage>,
    /// Page budget ran out before reaching a known alarm or the end.
    pub truncated: bool,
}

/// Calendar day of `now` in camera-local time.
#[must_use]
pub fn local_day(offset: Option<i32>, now: DateTime<Utc>) -> NaiveDate {
    offset.map_or_else(
        || now.with_timezone(&Local).date_naive(),
        |seconds| (now + chrono::Duration::seconds(i64::from(seconds))).date_naive(),
    )
}

/// UTC epoch seconds of the camera-local midnight starting `day`.
#[must_use]
pub fn local_midnight(day: NaiveDate, offset: Option<i32>, now: DateTime<Utc>) -> i64 {
    let seconds = offset.unwrap_or_else(|| now.with_timezone(&Local).offset().local_minus_utc());
    day.and_hms_opt(0, 0, 0)
        .map_or(0, |midnight| midnight.and_utc().timestamp())
        - i64::from(seconds)
}

#[must_use]
pub fn api_date(day: NaiveDate) -> String {
    day.format("%Y%m%d").to_string()
}

pub async fn fetch_unknown(
    source: &dyn AlarmSource,
    serial: &str,
    offset: Option<i32>,
    newest_known: Option<i64>,
    is_known: &(dyn Fn(&str) -> bool + Sync),
    max_pages: usize,
) -> Result<FetchResult, BridgeError> {
    let now = Utc::now();
    let today = local_day(offset, now);
    let mut collected: Vec<ParsedMessage> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    let mut reached = false;
    let mut exhausted_day = false;
    for (index, day) in [Some(today), today.checked_sub_days(Days::new(1))]
        .into_iter()
        .flatten()
        .enumerate()
    {
        if index == 1 {
            let gap = newest_known.is_some_and(|known| known < local_midnight(today, offset, now));
            if reached || pages >= max_pages || !exhausted_day || !gap {
                break;
            }
        }
        let date = api_date(day);
        let mut end_time = String::new();
        exhausted_day = false;
        while pages < max_pages {
            let page = source.alarm_list(serial, &date, &end_time).await?;
            pages += 1;
            let (messages, has_next) = parse_list(&page);
            let last_time = messages.last().and_then(|message| message.time_ms);
            let mut progressed = false;
            for message in messages {
                if is_known(&message.record.id) {
                    reached = true;
                } else if seen.insert(message.record.id.clone()) {
                    progressed = true;
                    collected.push(message);
                }
            }
            if reached {
                break;
            }
            match last_time {
                Some(time) if has_next && progressed => end_time = time.to_string(),
                _ => {
                    exhausted_day = !has_next;
                    break;
                }
            }
        }
    }
    let truncated = !reached && !exhausted_day && pages >= max_pages;
    collected.sort_by(|left, right| {
        (left.record.occurred_at, &left.record.id)
            .cmp(&(right.record.occurred_at, &right.record.id))
    });
    Ok(FetchResult {
        messages: collected,
        truncated,
    })
}
