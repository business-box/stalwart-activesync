//! Prometheus metrics in the text exposition format, kept by hand: a few
//! counters and gauges don't justify a metrics stack.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub struct Metrics {
    /// (command, HTTP status) -> (count, total seconds)
    requests: Mutex<BTreeMap<(String, u16), (u64, f64)>>,
    /// Requests currently held open by `Ping` or a waiting `Sync`.
    pub waiting: AtomicI64,
}

impl Metrics {
    pub fn record(&self, command: &str, status: u16, elapsed: Duration) {
        let mut m = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        let e = m.entry((command.to_string(), status)).or_default();
        e.0 += 1;
        e.1 += elapsed.as_secs_f64();
    }

    pub fn render(&self, devices: usize) -> String {
        let mut out = String::new();
        let m = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        out.push_str("# HELP eas_requests_total EAS requests by command and HTTP status.\n");
        out.push_str("# TYPE eas_requests_total counter\n");
        for ((cmd, status), (n, _)) in m.iter() {
            let _ = writeln!(
                out,
                "eas_requests_total{{command=\"{cmd}\",status=\"{status}\"}} {n}"
            );
        }
        out.push_str(
            "# HELP eas_request_seconds_total Time spent in EAS requests, including Ping waits.\n",
        );
        out.push_str("# TYPE eas_request_seconds_total counter\n");
        for ((cmd, status), (_, secs)) in m.iter() {
            let _ = writeln!(
                out,
                "eas_request_seconds_total{{command=\"{cmd}\",status=\"{status}\"}} {secs:.3}"
            );
        }
        out.push_str("# HELP eas_waiting_requests Ping and Sync requests waiting for changes.\n");
        out.push_str("# TYPE eas_waiting_requests gauge\n");
        let _ = writeln!(
            out,
            "eas_waiting_requests {}",
            self.waiting.load(Ordering::Relaxed)
        );
        out.push_str("# HELP eas_devices Devices known to the gateway.\n");
        out.push_str("# TYPE eas_devices gauge\n");
        let _ = writeln!(out, "eas_devices {devices}");
        out
    }
}

/// Counts a waiting request for as long as it lives.
pub struct WaitGuard<'a>(&'a AtomicI64);

impl<'a> WaitGuard<'a> {
    pub fn new(gauge: &'a AtomicI64) -> WaitGuard<'a> {
        gauge.fetch_add(1, Ordering::Relaxed);
        WaitGuard(gauge)
    }
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_counters_and_gauges() {
        let m = Metrics::default();
        m.record("Sync", 200, Duration::from_millis(1500));
        m.record("Sync", 200, Duration::from_millis(500));
        {
            let _w = WaitGuard::new(&m.waiting);
            assert!(m.render(3).contains("eas_waiting_requests 1"));
        }
        let text = m.render(3);
        assert!(text.contains("eas_requests_total{command=\"Sync\",status=\"200\"} 2"));
        assert!(text.contains("eas_request_seconds_total{command=\"Sync\",status=\"200\"} 2.000"));
        assert!(text.contains("eas_waiting_requests 0"));
        assert!(text.contains("eas_devices 3"));
    }
}
