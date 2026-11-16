//! Network free-space probing off the UI thread.
//!
//! A dead SMB share can block `GetDiskFreeSpaceExW` for ~30 s, so network
//! folders never query on the UI thread. [`ProbeWorker`] owns one
//! long-lived worker thread fed over a channel; [`Win32FreeProbe`] bounds
//! each query with a wall-clock budget (default 3 s) by running it on a
//! throwaway helper thread joined with `recv_timeout`. A timed-out helper
//! keeps running until the OS returns; its result is discarded. While a
//! root's helper is still alive, further probes of that root return
//! `None` at once instead of stacking up more stuck threads.
//! Pattern adapted from exbar's `reachability_probe.rs`.

use crate::config::VolumeId;
use crate::volumes;
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
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
    /// Roots whose helper thread has not returned yet.
    busy: Arc<Mutex<HashSet<String>>>,
}

impl Win32FreeProbe {
    /// [`volumes::query_free`] with the [`DEFAULT_PROBE_TIMEOUT`] budget.
    pub fn new() -> Self {
        Self::with_fn(volumes::query_free, DEFAULT_PROBE_TIMEOUT)
    }

    /// Injects the blocking query and budget (tests use a sleeping fn).
    pub fn with_fn(query: fn(&str) -> Option<u64>, timeout: Duration) -> Self {
        Win32FreeProbe {
            timeout,
            query,
            busy: Arc::default(),
        }
    }
}

impl Default for Win32FreeProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl FreeProbe for Win32FreeProbe {
    fn probe(&self, root: &str) -> Option<u64> {
        let root = root.to_owned();
        {
            let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
            if !busy.insert(root.clone()) {
                // A previous helper for this root is still stuck in the OS.
                return None;
            }
        }
        let (tx, rx) = mpsc::channel();
        let query = self.query;
        let busy = Arc::clone(&self.busy);
        std::thread::spawn(move || {
            // Declared after `tx` so it drops first: the root is freed
            // before the receiver can observe a reply or a disconnect,
            // even when `query` panics.
            let tx = tx;
            let guard = BusyGuard { busy, root };
            let free = query(&guard.root);
            drop(guard);
            // The receiver is gone after a timeout; the late result is dropped.
            let _ = tx.send(free);
        });
        rx.recv_timeout(self.timeout).ok().flatten()
    }
}

/// Removes `root` from the busy set when the helper finishes or unwinds.
struct BusyGuard {
    busy: Arc<Mutex<HashSet<String>>>,
    root: String,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.busy
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.root);
    }
}

/// Long-lived worker thread that runs probes one at a time and reports
/// results back to the UI thread.
pub struct ProbeWorker {
    tx: Sender<(VolumeId, String)>,
    rx: Receiver<(VolumeId, Option<u64>)>,
    in_flight: HashSet<VolumeId>,
    /// The worker thread's death has been logged.
    dead_logged: bool,
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
            dead_logged: false,
        }
    }

    /// Queues a probe of `root` for `id`; no-op while `id` is in flight.
    pub fn request(&mut self, id: VolumeId, root: String) {
        if self.in_flight.contains(&id) {
            return;
        }
        if self.tx.send((id.clone(), root)).is_ok() {
            self.in_flight.insert(id);
        } else {
            self.log_dead();
        }
    }

    /// Results ready so far (non-blocking); clears their in-flight marks.
    pub fn drain(&mut self) -> Vec<(VolumeId, Option<u64>)> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(r) => out.push(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.log_dead();
                    break;
                }
            }
        }
        for (id, _) in &out {
            self.in_flight.remove(id);
        }
        out
    }

    /// The worker thread exited (a probe panicked): network folders stop
    /// updating. Logged once.
    fn log_dead(&mut self) {
        if !self.dead_logged {
            self.dead_logged = true;
            log::warn!("net_probe: worker thread is gone; network folders will not refresh");
        }
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
        // notify() runs after the send, so the count may trail the results.
        let deadline = Instant::now() + Duration::from_secs(2);
        while notified.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
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
        std::thread::sleep(Duration::from_secs(3));
        Some(7)
    }

    #[test]
    fn timeout_yields_none() {
        let p = Win32FreeProbe::with_fn(slow_query, Duration::from_millis(100));
        let start = Instant::now();
        assert_eq!(p.probe(r"\\dead\share"), None);
        // Returns at the budget, far short of the 3 s query.
        assert!(start.elapsed() < Duration::from_millis(1500));
    }

    #[test]
    fn fast_query_within_budget_returns_value() {
        fn fast(_: &str) -> Option<u64> {
            Some(9)
        }
        let p = Win32FreeProbe::with_fn(fast, Duration::from_secs(10));
        assert_eq!(p.probe("X:\\"), Some(9));
    }
    static HANG_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn hang_query(_root: &str) -> Option<u64> {
        HANG_CALLS.fetch_add(1, Ordering::SeqCst);
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn hung_root_does_not_spawn_more_helpers() {
        let p = Win32FreeProbe::with_fn(hang_query, Duration::from_millis(100));
        assert_eq!(p.probe(r"\\dead\a"), None);
        // The first helper may start late under load; wait for it.
        assert!(wait_until(|| HANG_CALLS.load(Ordering::SeqCst) == 1));
        assert_eq!(p.probe(r"\\dead\a"), None);
        // A second helper would have bumped the count by now.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            HANG_CALLS.load(Ordering::SeqCst),
            1,
            "spawned another helper"
        );
    }

    /// Polls `cond` until true or 5 s pass.
    fn wait_until(cond: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    fn quick(_: &str) -> Option<u64> {
        Some(5)
    }

    #[test]
    fn finished_helper_frees_root_for_next_probe() {
        let p = Win32FreeProbe::with_fn(quick, Duration::from_secs(10));
        assert_eq!(p.probe("X:\\"), Some(5));
        assert_eq!(p.probe("X:\\"), Some(5));
    }

    static PANIC_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn panics_once(_: &str) -> Option<u64> {
        if PANIC_CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("query blew up");
        }
        Some(3)
    }

    #[test]
    fn panicking_query_frees_root() {
        let p = Win32FreeProbe::with_fn(panics_once, Duration::from_secs(10));
        assert_eq!(p.probe(r"\\boom\a"), None);
        assert_eq!(p.probe(r"\\boom\a"), Some(3), "root left busy after panic");
    }
}
