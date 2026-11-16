//! Binary entry point for drivecap.
//!
//! Two subcommands:
//!
//! - `drivecap run` — production (and the default when no subcommand is
//!   given). Started by the MSI's `HKCU\…\Run\Drivecap` entry at login
//!   and by the Start Menu shortcut. Calls `FreeConsole()` immediately so
//!   launches never flash a console window.
//! - `drivecap status` — diagnostic. Prints the volumes table to the
//!   console and exits.
//!
//! The runtime modules live in the `drivecap_cli` library crate; this
//! binary only orchestrates them.

// In release builds, mark this binary as Windows-subsystem so no console
// is allocated at launch. A console-subsystem binary, when launched via
// the MSI post-install action, the Run key, or a Start Menu shortcut,
// causes Windows Terminal (if set as default terminal) to briefly claim
// the console as a new tab — which then vanishes when `FreeConsole()`
// runs inside `run()`. Debug builds remain console-subsystem so
// `cargo run -- status` prints to the terminal during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::{Parser, Subcommand};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, FreeConsole};

use drivecap_cli::alerts::{Zone, zone_for};
use drivecap_cli::card_wnd;
use drivecap_cli::config::Config;
use drivecap_cli::log::{self as drivecap_log, LogLevel};
use drivecap_cli::model::format_free;
use drivecap_cli::paths;
use drivecap_cli::single_instance::{self, Instance};
use drivecap_cli::volumes::{self, VolumeSource, Win32Volumes};

/// Named mutex guarding the single running instance.
const SINGLE_INSTANCE_MUTEX: &str = r"Local\Drivecap-SingleInstance-v1";

// ── CLI definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "drivecap",
    version,
    about = "Always-on-top disk-space warner for Windows"
)]
struct Cli {
    /// Log verbosity for %TEMP%\drivecap.log
    #[arg(long, value_enum, default_value = "info")]
    log_level: LogLevel,

    /// No subcommand = `run` (this is how the MSI's WixShellExec
    /// launches us post-install).
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the tray app and warning card (default; started by the Run key)
    Run,
    /// Print the watched volumes and their free space
    Status,
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd.unwrap_or(Cmd::Run) {
        Cmd::Run => run(cli.log_level),
        Cmd::Status => status(),
    }
}

/// Production entry. Detaches from any inherited console first — no
/// `println!` after this point (it would silently no-op). A second
/// instance signals the first to show its card and exits 0.
fn run(level: LogLevel) {
    // Intentionally ignored: fails only when there is no console to free.
    let _ = unsafe { FreeConsole() };
    drivecap_log::init(level);
    volumes::suppress_critical_error_dialogs();
    log::info!("drivecap {} starting", env!("CARGO_PKG_VERSION"));
    let _guard = match single_instance::acquire(SINGLE_INSTANCE_MUTEX) {
        Instance::Primary(handle) => handle,
        Instance::Secondary => {
            log::info!("another instance is running; signalling it and exiting");
            single_instance::signal_primary();
            return;
        }
    };
    match card_wnd::run() {
        Ok(()) => log::info!("drivecap exiting"),
        Err(e) => {
            log::error!("drivecap stopped: {e}");
            std::process::exit(1);
        }
    }
}

/// Diagnostic table: watched volumes (`letter  label  free  zone  id`),
/// then connected volumes that are not watched. Network extras are
/// queried inline — a one-shot CLI may block on a dead share.
fn status() {
    // Release builds are Windows-subsystem; borrow the launching
    // terminal's console so `println!` lands there. Ignored: fails when
    // already attached or launched without a console.
    // SAFETY: no pointers; only changes this process's console attachment.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    volumes::suppress_critical_error_dialogs();
    let cfg = Config::load(&paths::config_path()).config;
    let source = Win32Volumes;
    let connected = source.connected();
    let extras: Vec<_> = cfg
        .extra_paths
        .iter()
        .filter_map(|p| source.resolve_path(p))
        .collect();
    let watched = volumes::select(&connected, &extras, &cfg);
    for v in &watched {
        let (free, zone) = match volumes::query_free(&v.root) {
            Some(bytes) => (format_free(bytes), zone_for(bytes, &cfg.rule_for(&v.id))),
            None => ("?".to_string(), Zone::Unreachable),
        };
        print_row(&v.letter, &v.label, &free, &format!("{zone:?}"), &v.id.0);
    }
    for r in connected
        .iter()
        .filter(|r| !watched.iter().any(|w| w.id == r.id))
    {
        let free = volumes::query_free(&r.root).map_or_else(|| "?".to_string(), format_free);
        print_row(&r.letter, &r.label, &free, "(not watched)", &r.id.0);
    }
}

fn print_row(letter: &str, label: &str, free: &str, zone: &str, id: &str) {
    println!("{letter:<3} {label:<20} {free:>9}  {zone:<13} {id}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_parses_as_default_run() {
        let cli = Cli::try_parse_from(["drivecap"]).expect("parses");
        assert!(cli.cmd.is_none());
    }

    #[test]
    fn log_level_and_status_subcommand_parse() {
        let cli =
            Cli::try_parse_from(["drivecap", "--log-level", "debug", "status"]).expect("parses");
        assert!(matches!(cli.cmd, Some(Cmd::Status)));
        assert!(matches!(cli.log_level, LogLevel::Debug));
    }
}
