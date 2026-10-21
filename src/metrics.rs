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
    /// Counters with their own fixed, low-cardinality labels.
    labeled: BTreeMap<(String, String), u64>,
}

impl Metrics {
    pub async fn increment(&self, name: &str, camera: &str) {
        self.add(name, camera, 1).await;
    }

    pub async fn add(&self, name: &str, camera: &str, amount: u64) {
        let mut values = self.inner.lock().await;
        let counter = values
            .counters
            .entry((name.to_owned(), camera.to_owned()))
            .or_default();
        *counter = counter.saturating_add(amount);
    }

    /// Counter whose labels are static names and values, never vendor data.
    pub async fn increment_labeled(&self, name: &str, labels: &[(&str, &str)]) {
        let rendered = labels
            .iter()
            .map(|(key, value)| format!("{key}=\"{}\"", escape_label(value)))
            .collect::<Vec<_>>()
            .join(",");
        let mut values = self.inner.lock().await;
        let counter = values
            .labeled
            .entry((name.to_owned(), rendered))
            .or_default();
        *counter = counter.saturating_add(1);
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
        for ((name, labels), value) in &values.labeled {
            let _result = if labels.is_empty() {
                writeln!(output, "ezviz_bridge_{name} {value}")
            } else {
                writeln!(output, "ezviz_bridge_{name}{{{labels}}} {value}")
            };
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

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[tokio::test]
    async fn labeled_counters_render_their_labels() {
        let metrics = Metrics::default();
        let labels = [("read", "snapshot"), ("category", "upstream")];
        metrics
            .increment_labeled("control_read_failures_total", &labels)
            .await;
        metrics
            .increment_labeled("control_read_failures_total", &labels)
            .await;
        assert!(metrics.render().await.contains(
            "ezviz_bridge_control_read_failures_total{read=\"snapshot\",category=\"upstream\"} 2"
        ));
    }
}
