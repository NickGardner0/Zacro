use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const MAX_LATENCY_SAMPLES: usize = 2_048;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetricsSnapshot {
    pub input_to_paint_p50_ms: f64,
    pub input_to_paint_p95_ms: f64,
    pub input_to_paint_sample_count: usize,
    pub render_build_p50_ms: f64,
    pub render_build_p95_ms: f64,
    pub render_build_sample_count: usize,
    pub slow_render_builds: u64,
    pub top_level_requests: u64,
    pub cache_hits: u64,
    pub reconnects: u64,
}

#[derive(Debug, Default)]
pub struct PerformanceMetrics {
    input_to_paint: Mutex<VecDeque<Duration>>,
    render_build: Mutex<VecDeque<Duration>>,
    slow_render_builds: AtomicU64,
    top_level_requests: AtomicU64,
    cache_hits: AtomicU64,
    reconnects: AtomicU64,
}

impl PerformanceMetrics {
    pub fn record_input_to_paint(&self, duration: Duration) {
        record_bounded_sample(&self.input_to_paint, duration);
    }

    /// Records synchronous GPUI element-tree construction. This is not GPU
    /// paint time; it identifies expensive render functions and accidental
    /// cloning before layout reaches the platform compositor.
    pub fn record_render_build(&self, duration: Duration) {
        record_bounded_sample(&self.render_build, duration);
        if duration > Duration::from_millis(8) {
            self.slow_render_builds.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn clear_latency_samples(&self) {
        self.input_to_paint.lock().unwrap().clear();
        self.render_build.lock().unwrap().clear();
        self.slow_render_builds.store(0, Ordering::Relaxed);
    }

    pub fn record_top_level_request(&self) {
        self.top_level_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_reconnect(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let input_to_paint = sorted_milliseconds(&self.input_to_paint);
        let render_build = sorted_milliseconds(&self.render_build);
        MetricsSnapshot {
            input_to_paint_p50_ms: percentile(&input_to_paint, 0.50),
            input_to_paint_p95_ms: percentile(&input_to_paint, 0.95),
            input_to_paint_sample_count: input_to_paint.len(),
            render_build_p50_ms: percentile(&render_build, 0.50),
            render_build_p95_ms: percentile(&render_build, 0.95),
            render_build_sample_count: render_build.len(),
            slow_render_builds: self.slow_render_builds.load(Ordering::Relaxed),
            top_level_requests: self.top_level_requests.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
        }
    }
}

fn record_bounded_sample(samples: &Mutex<VecDeque<Duration>>, duration: Duration) {
    let mut samples = samples.lock().unwrap();
    if samples.len() == MAX_LATENCY_SAMPLES {
        samples.pop_front();
    }
    samples.push_back(duration);
}

fn sorted_milliseconds(samples: &Mutex<VecDeque<Duration>>) -> Vec<f64> {
    let mut samples = samples
        .lock()
        .unwrap()
        .iter()
        .map(Duration::as_secs_f64)
        .map(|seconds| seconds * 1_000.0)
        .collect::<Vec<_>>();
    samples.sort_by(f64::total_cmp);
    samples
}

fn percentile(samples: &[f64], percentile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() - 1) as f64 * percentile).ceil() as usize;
    samples[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_bounded_latency_percentiles_and_counters() {
        let metrics = PerformanceMetrics::default();
        for milliseconds in 1..=100 {
            metrics.record_input_to_paint(Duration::from_millis(milliseconds));
        }
        metrics.record_top_level_request();
        metrics.record_cache_hit();
        metrics.record_reconnect();
        metrics.record_render_build(Duration::from_millis(4));
        metrics.record_render_build(Duration::from_millis(12));
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.input_to_paint_p50_ms, 51.0);
        assert_eq!(snapshot.input_to_paint_p95_ms, 96.0);
        assert_eq!(snapshot.render_build_p50_ms, 12.0);
        assert_eq!(snapshot.render_build_p95_ms, 12.0);
        assert_eq!(snapshot.render_build_sample_count, 2);
        assert_eq!(snapshot.slow_render_builds, 1);
        assert_eq!(snapshot.top_level_requests, 1);
        assert_eq!(snapshot.cache_hits, 1);
        assert_eq!(snapshot.reconnects, 1);

        metrics.clear_latency_samples();
        let cleared = metrics.snapshot();
        assert_eq!(cleared.input_to_paint_sample_count, 0);
        assert_eq!(cleared.render_build_sample_count, 0);
        assert_eq!(cleared.slow_render_builds, 0);
        assert_eq!(cleared.top_level_requests, 1);
    }
}
