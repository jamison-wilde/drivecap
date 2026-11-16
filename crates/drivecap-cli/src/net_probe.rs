//! Network free-space probing off the UI thread.
//!
//! A dead SMB share can block `GetDiskFreeSpaceExW` for ~30 s, so network
//! folders never query on the UI thread. [`ProbeWorker`] owns one
//! long-lived worker thread fed over a channel; [`Win32FreeProbe`] bounds
//! each query with a wall-clock budget (default 3 s) by running it on a
//! throwaway helper thread joined with `recv_timeout`. A timed-out helper
//! keeps running until the OS returns; its result is discarded.
//! Pattern adapted from exbar's `reachability_probe.rs`.

use crate::config::VolumeId;
use crate::volumes;
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Default per-probe wall-clock budget.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Synchronous free-space probe (ADR-0002 seam). Implementations bound
/// their own runtime.
pub trait FreeProbe: Send + Sync {
    /// Free bytes at `root`; `None` = unreachable, failed or timed out.
    fn probe(&self, root: &str) -> Option<u64>;
}

/// Production probe: [`volumes::query_free`] on a helper thread, bounded
/// by `timeout`.
pub struct Win32FreeProbe {
    timeout: Duration,
    query: fn(&str) -> Option<u64>,
}

impl Win32FreeProbe {
    /// [`volumes::query_free`] with the [`DEFAULT_PROBE_TIMEOUT`] budget.
    pub fn new() -> Self {
        Self::with_fn(volumes::query_free, DEFAULT_PROBE_TIMEOUT)
    }

    /// Injects the blocking query and budget (tests use a sleeping fn).
    pub fn with_fn(query: fn(&str) -> Option<u64>, timeout: Duration) -> Self {
        Win32FreeProbe { timeout, query }
    }
}

impl Default for Win32FreeProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl FreeProbe for Win32FreeProbe {
    fn probe(&self, root: &str) -> Option<u64> {
        let (tx, rx) = mpsc::channel();
        let root = root.to_owned();
        let query = self.query;
        std::thread::spawn(move || {
            // The receiver is gone after a timeout; the late result is dropped.
            let _ = tx.send(query(&root));
        });
        rx.recv_timeout(self.timeout).ok().flatten()
    }
}

/// Long-lived worker thread that runs probes one at a time and reports
/// results back to the UI thread.
pub struct ProbeWorker {
    tx: Sender<(VolumeId, String)>,
    rx: Receiver<(VolumeId, Option<u64>)>,
    in_flight: HashSet<VolumeId>,
}

impl ProbeWorker {
    /// Starts the worker. `notify` runs on the worker thread after each
    /// result is queued (production: `PostMessageW(card, WM_APP_PROBE_DONE)`).
    /// Dropping the worker closes the request channel and the thread exits
    /// after its current probe.
    pub fn spawn(probe: Box<dyn FreeProbe>, notify: Box<dyn Fn() + Send>) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<(VolumeId, String)>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((id, root)) = req_rx.recv() {
                let free = probe.probe(&root);
                if res_tx.send((id, free)).is_err() {
                    return;
                }
                notify();
            }
        });
        ProbeWorker {
            tx: req_tx,
            rx: res_rx,
            in_flight: HashSet::new(),
        }
    }

    /// Queues a probe of `root` for `id`; no-op while `id` is in flight.
    pub fn request(&mut self, id: VolumeId, root: String) {
        if self.in_flight.contains(&id) {
            return;
        }
        if self.tx.send((id.clone(), root)).is_ok() {
            self.in_flight.insert(id);
        }
    }

    /// Results ready so far (non-blocking); clears their in-flight marks.
    pub fn drain(&mut self) -> Vec<(VolumeId, Option<u64>)> {
        let out: Vec<_> = self.rx.try_iter().collect();
        for (id, _) in &out {
            self.in_flight.remove(id);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[derive(Default)]
    struct MockProbe {
        results: HashMap<String, Option<u64>>,
        delay: Option<Duration>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl FreeProbe for MockProbe {
        fn probe(&self, root: &str) -> Option<u64> {
            self.calls.lock().unwrap().push(root.to_owned());
            if let Some(d) = self.delay {
                std::thread::sleep(d);
            }
            self.results.get(root).copied().flatten()
        }
    }

    fn id(s: &str) -> VolumeId {
        VolumeId(s.to_owned())
    }

    /// Drains until `n` results arrive or 2 s pass.
    fn drain_n(w: &mut ProbeWorker, n: usize) -> Vec<(VolumeId, Option<u64>)> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut out = Vec::new();
        while out.len() < n && Instant::now() < deadline {
            out.extend(w.drain());
            std::thread::sleep(Duration::from_millis(5));
        }
        out
    }

    #[test]
    fn result_delivered_and_notified() {
        let mut results = HashMap::new();
        results.insert(r"\\nas\a".to_owned(), Some(42));
        let probe = MockProbe {
            results,
            ..MockProbe::default()
        };
        let notified = Arc::new(AtomicUsize::new(0));
        let n = notified.clone();
        let mut w = ProbeWorker::spawn(
            Box::new(probe),
            Box::new(move || {
                n.fetch_add(1, Ordering::SeqCst);
            }),
        );
        w.request(id("a"), r"\\nas\a".into());
        w.request(id("b"), r"\\nas\b".into());
        let mut got = drain_n(&mut w, 2);
        got.sort();
        assert_eq!(got, vec![(id("a"), Some(42)), (id("b"), None)]);
        assert_eq!(notified.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn duplicate_request_while_in_flight_ignored() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let probe = MockProbe {
            delay: Some(Duration::from_millis(100)),
            calls: calls.clone(),
            ..MockProbe::default()
        };
        let mut w = ProbeWorker::spawn(Box::new(probe), Box::new(|| {}));
        w.request(id("a"), "A".into());
        w.request(id("a"), "A".into());
        assert_eq!(drain_n(&mut w, 1).len(), 1);
        std::thread::sleep(Duration::from_millis(150));
        assert!(w.drain().is_empty(), "duplicate was probed");
        assert_eq!(calls.lock().unwrap().len(), 1);
        // Once drained, the id may be requested again.
        w.request(id("a"), "A".into());
        assert_eq!(drain_n(&mut w, 1).len(), 1);
    }

    fn slow_query(_root: &str) -> Option<u64> {
        std::thread::sleep(Duration::from_millis(500));
        Some(7)
    }

    #[test]
    fn timeout_yields_none() {
        let p = Win32FreeProbe::with_fn(slow_query, Duration::from_millis(50));
        let start = Instant::now();
        assert_eq!(p.probe(r"\\dead\share"), None);
        assert!(start.elapsed() < Duration::from_millis(400));
    }

    #[test]
    fn fast_query_within_budget_returns_value() {
        fn fast(_: &str) -> Option<u64> {
            Some(9)
        }
        let p = Win32FreeProbe::with_fn(fast, Duration::from_millis(500));
        assert_eq!(p.probe("X:\\"), Some(9));
    }
}
