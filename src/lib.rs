//! Zenless Download Manager core library.
//!
//! Everything that does not draw pixels lives here so it can be unit- and
//! integration-tested without a window:
//!
//! * [`engine`] – the tokio-based download engine (probing, IDM-style dynamic
//!   segmentation, queue, speed limits, persistence).
//! * [`api`] – the local HTTP API used by the browser extensions (port 6812).
//! * [`settings`], [`category`], [`filename`] – configuration and helpers.
//! * [`clipboard`], [`autostart`] – small OS integrations.
//!
//! The egui front-end lives in the binary crate (`src/main.rs`, `src/ui/`).

pub mod api;
pub mod autostart;
pub mod category;
pub mod clipboard;
pub mod demo;
pub mod engine;
pub mod filename;
pub mod settings;
pub mod util;

/// Product name shown in the UI and returned by the API.
pub const APP_NAME: &str = "Zenless Download Manager";
/// Short application id used by the API (`"app"` field).
pub const APP_ID: &str = "zenless-dm";
/// Crate version (0.1.0).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Product website.
pub const WEBSITE: &str = "https://zenless-suite.vercel.app";
/// Download page for the apps and browser extensions.
pub const DOWNLOAD_PAGE: &str = "https://zenless-suite.vercel.app/download";
/// Source repository.
pub const REPOSITORY: &str = "https://github.com/zenless-inc/zenless-download-manager";
