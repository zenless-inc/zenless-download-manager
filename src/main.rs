#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Zenless Download Manager – entry point.
//!
//! `zenless-dm.exe [--minimized] [URL...]`
//!
//! Single instance: if another instance answers on the local API port, the
//! URLs are forwarded to it (`POST /download` / `/batch`), it is focused and
//! this process exits.
//!
//! Environment (debug aids, all harmless):
//! * `ZENLESS_DEMO=1` – fake, animated sample downloads; no network, no API
//!   server, nothing is saved. `ZENLESS_DEMO_VIEW=new|batch|properties|delete|
//!   settings[-network|-browser|-appearance|-about]|empty` opens that screen.
//! * `ZENLESS_SCREENSHOT=<file.png>` – save a screenshot after a few frames and exit.

// The shared UI kit is copied verbatim into every Zenless app; not every
// helper is used here.
#[allow(dead_code)]
mod shared;
mod ui;

use eframe::egui;
use serde_json::json;
use std::time::Duration;
use zenless_dm::{api, engine, settings};

/// Parsed command line.
#[derive(Debug, Default, Clone)]
pub struct Cli {
    pub minimized: bool,
    pub urls: Vec<String>,
}

fn parse_cli() -> Cli {
    let mut cli = Cli::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--minimized" | "-m" | "/minimized" => cli.minimized = true,
            a if zenless_dm::util::looks_like_url(a) => cli.urls.push(a.trim().to_owned()),
            _ => {}
        }
    }
    cli
}

/// Hands our arguments to the running instance. Returns `true` on success.
fn forward_to_running_instance(cli: &Cli) -> bool {
    let t = Duration::from_millis(1500);
    let post = |path: &str, body: String| api::client_request(api::PORT, "POST", path, Some(&body), t);
    match cli.urls.len() {
        0 => {}
        1 => {
            let body = json!({ "url": cli.urls[0], "source": "cli", "mode": "ask" }).to_string();
            if post("/download", body).is_err() {
                return false;
            }
        }
        _ => {
            let items: Vec<_> = cli.urls.iter().map(|u| json!({ "url": u })).collect();
            let body = json!({ "items": items, "source": "cli" }).to_string();
            if post("/batch", body).is_err() {
                return false;
            }
        }
    }
    if !cli.minimized {
        let _ = post("/focus", "{}".into());
    }
    true
}

fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/icon-128.rgba").to_vec(),
        width: 128,
        height: 128,
    }
}

fn main() -> eframe::Result {
    let cli = parse_cli();
    let demo = std::env::var("ZENLESS_DEMO").is_ok_and(|v| v == "1");
    let screenshot = std::env::var_os("ZENLESS_SCREENSHOT").is_some();

    if !demo && !screenshot && api::ping_existing(api::PORT, Duration::from_millis(300)) && forward_to_running_instance(&cli) {
        return Ok(());
    }

    let data_dir = settings::data_dir();
    let config = if demo {
        let settings = engine::store::load_settings(&data_dir);
        let downloads = if std::env::var("ZENLESS_DEMO_VIEW").is_ok_and(|v| v == "empty") {
            Vec::new()
        } else {
            zenless_dm::demo::sample_downloads(&settings.download_dir)
        };
        engine::EngineConfig { settings, downloads, data_dir: None, demo: true }
    } else {
        engine::EngineConfig::load(&data_dir)
    };

    let viewport = egui::ViewportBuilder::default()
        .with_title(zenless_dm::APP_NAME)
        .with_app_id("zenless-dm")
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([760.0, 460.0])
        .with_icon(window_icon())
        .with_drag_and_drop(true);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "zenless-dm",
        options,
        Box::new(move |cc| {
            ui::App::new(cc, config, cli, demo)
                .map(|app| Box::new(app) as Box<dyn eframe::App>)
                .map_err(Into::into)
        }),
    )
}
