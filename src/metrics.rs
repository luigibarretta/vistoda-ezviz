use std::{collections::BTreeMap, fmt::Write as _, sync::Arc};

use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct Metrics {
    inner: Arc<Mutex<MetricValues>>,
}

#[derive(Default)]
struct MetricValues {
    counters: BTreeMap<(String, String), u64>,
    gauges: BTreeMap<(String, String), f64>,
}

impl Metrics {
    pub async fn increment(&self, name: &str, camera: &str) {
        let mut values = self.inner.lock().await;
        *values
            .counters
            .entry((name.to_owned(), camera.to_owned()))
            .or_default() += 1;
    }

    pub async fn gauge(&self, name: &str, camera: &str, value: f64) {
        self.inner
            .lock()
            .await
            .gauges
            .insert((name.to_owned(), camera.to_owned()), value);
    }

    pub async fn render(&self) -> String {
        let values = self.inner.lock().await;
        let mut output = String::new();
        for ((name, camera), value) in &values.counters {
            let _result = writeln!(
                output,
                "ezviz_bridge_{name}{{camera=\"{}\"}} {value}",
                escape_label(camera)
            );
        }
        for ((name, camera), value) in &values.gauges {
            let _result = writeln!(
                output,
                "ezviz_bridge_{name}{{camera=\"{}\"}} {value}",
                escape_label(camera)
            );
        }
        output
    }
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
