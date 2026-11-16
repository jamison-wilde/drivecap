//! Filesystem layout for drivecap's persisted state.
//!
//! All state lives under `~/.drivecap/`:
//! - `config.json` — volume rules and runtime options.
//! - `position.json` — saved card position (per monitor).
//!
//! Drivecap has no legacy layout, so there is no migration step. Don't
//! add state files outside `~/.drivecap/` — keep the layout one folder.
//! The log is the one exception: it lives at `%TEMP%\drivecap.log`
//! (see [`crate::log`]).

use std::path::PathBuf;

/// `~/.drivecap/` — the directory that holds all of drivecap's persisted state.
pub fn drivecap_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| String::from("C:\\Users\\Default"));
    let mut p = PathBuf::from(home);
    p.push(".drivecap");
    p
}

/// `~/.drivecap/config.json` — volume rules + runtime options.
pub fn config_path() -> PathBuf {
    let mut p = drivecap_dir();
    p.push("config.json");
    p
}

/// `~/.drivecap/position.json` — saved card position.
pub fn position_path() -> PathBuf {
    let mut p = drivecap_dir();
    p.push("position.json");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drivecap_dir_ends_with_dot_drivecap() {
        let p = drivecap_dir();
        assert!(p.ends_with(".drivecap"), "expected .drivecap suffix: {p:?}");
    }

    #[test]
    fn config_path_under_drivecap_dir() {
        let p = config_path();
        assert_eq!(p.file_name().unwrap().to_str(), Some("config.json"));
        assert!(p.parent().unwrap().ends_with(".drivecap"));
    }

    #[test]
    fn position_path_under_drivecap_dir() {
        let p = position_path();
        assert_eq!(p.file_name().unwrap().to_str(), Some("position.json"));
        assert!(p.parent().unwrap().ends_with(".drivecap"));
    }
}
