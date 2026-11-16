//! File-backed logger driven by the `log` crate.
//!
//! Verbosity comes from the `--log-level` CLI flag (defaults to Info).
//! Messages below the configured filter are not formatted, keeping
//! hot-path cost minimal when verbose logging is off. Output goes to
//! `%TEMP%\drivecap.log`, one line per record:
//! `HH:MM:SS.mmm [LEVEL] pid=N message`. At startup a log over
//! [`MAX_LOG_BYTES`] is moved to `drivecap.log.1` (replacing the previous
//! one), so the pair never exceeds about twice the limit per run.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Size past which [`init`] rotates the log to `drivecap.log.1`.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Log verbosity. Parsed from the `--log-level` CLI flag (lowercase
/// names) and mapped onto [`log::LevelFilter`] by [`init`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// The `log` crate filter this level enables.
    pub fn to_filter(self) -> log::LevelFilter {
        match self {
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Trace => log::LevelFilter::Trace,
        }
    }
}

/// `%TEMP%\drivecap.log` — the single log file.
pub fn log_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("drivecap.log");
    p
}

struct FileLogger;

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
        {
            let pid = std::process::id();
            let ts = timestamp();
            let level = format_level(record.level());
            // Intentionally ignored: logging-inside-logger must not recurse.
            let _ = writeln!(f, "{ts} [{level}] pid={pid} {}", record.args());
        }
    }

    fn flush(&self) {}
}

static FILE_LOGGER: FileLogger = FileLogger;

/// Install the file logger and set the max level.
///
/// Idempotent: a second call only updates the level (`log::set_logger`
/// errors are ignored because it fails only if a logger is already set).
pub fn init(level: LogLevel) {
    let rotated = rotate_if_large(&log_path(), MAX_LOG_BYTES);
    let _ = log::set_logger(&FILE_LOGGER);
    log::set_max_level(level.to_filter());
    if rotated {
        log::info!("log: rotated the previous log to drivecap.log.1");
    }
}

/// Moves `path` to `<path>.1` (replacing it) when `path` is larger than
/// `max_bytes`. Returns whether it rotated; a missing file or a failed
/// rename (another process writing) leaves the log where it is.
fn rotate_if_large(path: &Path, max_bytes: u64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() <= max_bytes {
        return false;
    }
    let mut old = path.as_os_str().to_owned();
    old.push(".1");
    // std::fs::rename replaces an existing target on Windows.
    std::fs::rename(path, PathBuf::from(old)).is_ok()
}

fn format_level(level: log::Level) -> &'static str {
    // 5-char padded so the message column aligns.
    match level {
        log::Level::Error => "ERROR",
        log::Level::Warn => "WARN ",
        log::Level::Info => "INFO ",
        log::Level::Debug => "DEBUG",
        log::Level::Trace => "TRACE",
    }
}

fn timestamp() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format_timestamp(now.as_secs(), now.subsec_millis())
}

/// `HH:MM:SS.mmm` (UTC) for a unix time — good enough for log correlation.
fn format_timestamp(total_secs: u64, millis: u32) -> String {
    let secs_in_day = total_secs % 86400;
    let h = secs_in_day / 3600;
    let m = (secs_in_day % 3600) / 60;
    let s = secs_in_day % 60;
    format!("{h:02}:{m:02}:{s:02}.{millis:03}")
}

/// Log a warning (at debug level) if the expression is Err; discard
/// the value either way. `stringify!($e)` captures the call text for
/// log context.
///
/// The expression must evaluate to a `Result<_, impl Display>`. For Win32
/// BOOL-returning calls, use `.ok()` to convert to `windows::core::Result<()>`:
///
/// ```ignore
/// // Windows BOOL-returning call:
/// warn_on_err!(unsafe { ShowWindow(hwnd, SW_HIDE).ok() });
///
/// // Already returns Result:
/// warn_on_err!(unsafe { DestroyWindow(hwnd) });
/// ```
#[macro_export]
macro_rules! warn_on_err {
    ($e:expr) => {
        match ($e) {
            Ok(v) => {
                let _ = v;
            }
            Err(err) => {
                log::debug!("{}: {}", stringify!($e), err);
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_is_drivecap_log_in_temp() {
        let p = log_path();
        assert_eq!(p.file_name().unwrap().to_str(), Some("drivecap.log"));
        assert_eq!(p.parent().unwrap(), std::env::temp_dir().as_path());
    }

    #[test]
    fn level_maps_to_matching_filter() {
        assert_eq!(LogLevel::Error.to_filter(), log::LevelFilter::Error);
        assert_eq!(LogLevel::Warn.to_filter(), log::LevelFilter::Warn);
        assert_eq!(LogLevel::Info.to_filter(), log::LevelFilter::Info);
        assert_eq!(LogLevel::Debug.to_filter(), log::LevelFilter::Debug);
        assert_eq!(LogLevel::Trace.to_filter(), log::LevelFilter::Trace);
    }

    #[test]
    fn default_level_is_info() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }

    #[test]
    fn level_parses_from_lowercase_cli_value() {
        use clap::ValueEnum as _;
        assert_eq!(LogLevel::from_str("debug", false), Ok(LogLevel::Debug));
        assert!(LogLevel::from_str("verbose", false).is_err());
    }

    #[test]
    fn format_level_is_five_chars() {
        for l in [
            log::Level::Error,
            log::Level::Warn,
            log::Level::Info,
            log::Level::Debug,
            log::Level::Trace,
        ] {
            assert_eq!(format_level(l).len(), 5);
        }
    }

    #[test]
    fn timestamp_formats_time_of_day() {
        // 1 day + 01:02:03 → day part dropped.
        assert_eq!(format_timestamp(86400 + 3723, 45), "01:02:03.045");
    }

    #[test]
    fn oversized_log_rotates_to_dot_1_replacing_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("drivecap.log");
        let old = dir.path().join("drivecap.log.1");
        std::fs::write(&log, vec![b'x'; 11]).unwrap();
        std::fs::write(&old, b"stale").unwrap();
        assert!(rotate_if_large(&log, 10));
        assert!(!log.exists());
        assert_eq!(std::fs::read(&old).unwrap().len(), 11);
    }

    #[test]
    fn log_at_or_under_the_limit_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("drivecap.log");
        std::fs::write(&log, vec![b'x'; 10]).unwrap();
        assert!(!rotate_if_large(&log, 10));
        assert!(log.exists());
        assert!(!dir.path().join("drivecap.log.1").exists());
    }

    #[test]
    fn missing_log_is_not_rotated() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!rotate_if_large(&dir.path().join("drivecap.log"), 10));
    }

    #[test]
    fn rotation_limit_is_5_mb() {
        assert_eq!(MAX_LOG_BYTES, 5 * 1024 * 1024);
    }
}
