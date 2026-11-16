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
use windows::Win32::System::Console::FreeConsole;

use drivecap_cli::log::{self as drivecap_log, LogLevel};

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
/// `println!` after this point (it would silently no-op).
fn run(level: LogLevel) {
    // Intentionally ignored: fails only when there is no console to free.
    let _ = unsafe { FreeConsole() };
    drivecap_log::init(level);
    log::info!("drivecap {} starting", env!("CARGO_PKG_VERSION"));
    // Message pump arrives with the card window (Task 12).
}

fn status() {
    // Volume enumeration arrives with the volumes module (Task 8).
    println!("status: not implemented");
}
