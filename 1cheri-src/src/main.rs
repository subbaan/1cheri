// 1cheri v0.4.27 — feature: a "?" button next to Add in Settings' Boards section opens a searchable directory of every real 4chan board (fetched live), showing title/description and marking 18+ boards -- click one to add it directly.

mod comments;
mod config;
mod filters;
mod logging;
mod media_cache;
mod models;
mod net;
mod player;
mod storage;
mod thumbnails;
mod ui;

use config::Config;
use gtk4::prelude::*;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("1cheri {VERSION}");
    println!("Keyboard-driven viewer for 4chan image/video threads.");
    println!();
    println!("Usage: 1cheri [OPTIONS]");
    println!();
    println!("Options:");
    println!("  -h, --help     Print this help and exit");
    println!("  -v, --version  Print version and exit");
    println!("      --fixture  Open the bundled offline sample thread instead of");
    println!("                 fetching a real board (for local dev/testing)");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-v" || a == "--version") {
        println!("1cheri {VERSION}");
        return ExitCode::SUCCESS;
    }
    let open_fixture = args.iter().any(|a| a == "--fixture");

    logging::redirect_stdio_to_log_file();

    let config = Config::load();
    let fixture_media_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/media"));

    // Force a dark theme regardless of the desktop's GTK theme: GTK_THEME
    // must be set before GTK reads it at init time -- setting it via
    // gtk4::Settings after the fact doesn't reliably override XFCE's
    // xsettings daemon, which keeps pushing the system theme.
    unsafe {
        env::set_var("GTK_THEME", "Adwaita:dark");
    }

    let app = gtk4::Application::builder()
        .application_id("org.cheri.app")
        .build();

    app.connect_activate(move |app| {
        // GApplication treats a fixed application_id as single-instance by
        // default: launching a second process while one is already running
        // doesn't start a separate process, it just re-fires `activate` in
        // the *existing* one over D-Bus. Without this check, that silently
        // built a second top-level window inside the already-running
        // instance every time -- indistinguishable from the app actually
        // launching twice, and easy to end up with several stray windows
        // piling up with no obvious cause.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        ui::build_window(app, config.clone(), fixture_media_dir.clone(), open_fixture);
    });

    let empty: [&str; 0] = [];
    app.run_with_args(&empty);
    ExitCode::SUCCESS
}
