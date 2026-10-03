//! User settings (`%APPDATA%\Zenless\DownloadManager\settings.json`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default browser-like user agent (servers are friendlier to it).
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 ZenlessDM/1.0";

pub const MIN_CONNECTIONS: u32 = 1;
pub const MAX_CONNECTIONS: u32 = 32;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Default save folder.
    pub download_dir: PathBuf,
    /// Save into `<folder>\<Category>\` sub-folders.
    pub category_subfolders: bool,
    /// Queue: how many downloads run at the same time.
    pub max_concurrent: u32,
    /// Connections per new download (1–32).
    pub default_connections: u32,
    /// Use fewer connections for small files: many sites rate-limit or block
    /// clients that open lots of connections for one small download.
    pub small_file_limit: bool,
    /// Files up to this size (MiB) count as small.
    pub small_file_mb: u64,
    /// Connections for a small file (when you didn't pick a number yourself).
    pub small_file_connections: u32,
    /// Mirrors the HKCU Run key (the registry is the source of truth).
    pub start_with_windows: bool,
    /// Resume downloads that were running when the app was closed.
    pub auto_resume: bool,
    pub confirm_delete: bool,
    /// Offer to download URLs copied to the clipboard.
    pub clipboard_monitor: bool,
    /// Toasts + taskbar flash when downloads finish.
    pub notifications: bool,
    /// Global speed limit.
    pub speed_limit_enabled: bool,
    /// Global speed limit in KiB/s.
    pub speed_limit_kib: u64,
    pub user_agent: String,
    /// Retries per connection before it gives up.
    pub max_retries: u32,
    /// Connect / read timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            category_subfolders: false,
            max_concurrent: 3,
            default_connections: 8,
            small_file_limit: true,
            small_file_mb: 100,
            small_file_connections: 2,
            start_with_windows: false,
            auto_resume: false,
            confirm_delete: true,
            clipboard_monitor: false,
            notifications: true,
            speed_limit_enabled: false,
            speed_limit_kib: 2048,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            max_retries: 8,
            timeout_secs: 30,
        }
    }
}

impl Settings {
    /// Clamps every value into its valid range.
    pub fn normalized(mut self) -> Self {
        self.max_concurrent = self.max_concurrent.clamp(1, 16);
        self.default_connections = self.default_connections.clamp(MIN_CONNECTIONS, MAX_CONNECTIONS);
        self.small_file_mb = self.small_file_mb.clamp(1, 100_000);
        self.small_file_connections = self.small_file_connections.clamp(MIN_CONNECTIONS, MAX_CONNECTIONS);
        self.max_retries = self.max_retries.min(50);
        self.timeout_secs = self.timeout_secs.clamp(5, 300);
        self.speed_limit_kib = self.speed_limit_kib.clamp(16, 10 * 1024 * 1024);
        if self.user_agent.trim().is_empty() {
            self.user_agent = DEFAULT_USER_AGENT.to_owned();
        }
        if self.download_dir.as_os_str().is_empty() {
            self.download_dir = default_download_dir();
        }
        self
    }

    /// The small-file rule for downloads whose connection count wasn't chosen
    /// by hand, or `None` when it's turned off.
    pub fn small_file_policy(&self) -> Option<crate::engine::segments::SmallFilePolicy> {
        self.small_file_limit.then(|| crate::engine::segments::SmallFilePolicy {
            max_bytes: self.small_file_mb.saturating_mul(1024 * 1024),
            connections: self.small_file_connections,
        })
    }

    /// Effective global limit in bytes per second (`0` = unlimited).
    pub fn global_limit_bps(&self) -> u64 {
        if self.speed_limit_enabled {
            self.speed_limit_kib.saturating_mul(1024)
        } else {
            0
        }
    }
}

pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `%APPDATA%\Zenless\DownloadManager` (`ZENLESS_DM_DATA_DIR` overrides it,
/// for tests that must not touch the real data).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ZENLESS_DM_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Zenless")
        .join("DownloadManager")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_clamping() {
        let s = Settings::default();
        assert_eq!(s.max_concurrent, 3);
        assert_eq!(s.default_connections, 8);
        assert_eq!(s.max_retries, 8);
        let s = Settings { default_connections: 99, max_concurrent: 0, ..Settings::default() }.normalized();
        assert_eq!(s.default_connections, 32);
        assert_eq!(s.max_concurrent, 1);
        // Unknown / missing fields fall back to defaults.
        let s: Settings = serde_json::from_str(r#"{"max_concurrent":5,"bogus":1}"#).unwrap();
        assert_eq!(s.max_concurrent, 5);
        assert_eq!(s.default_connections, 8);
    }

    #[test]
    fn small_file_rule() {
        // On by default (settings files from 0.2.0 don't have the keys yet): ≤ 100 MiB → 2 connections.
        let s: Settings = serde_json::from_str(r#"{"default_connections":8}"#).unwrap();
        let p = s.small_file_policy().expect("on by default");
        assert_eq!(p.max_bytes, 100 * 1024 * 1024);
        assert_eq!(p.connections, 2);
        assert!(Settings { small_file_limit: false, ..Settings::default() }.small_file_policy().is_none());
        let s = Settings { small_file_mb: 0, small_file_connections: 0, ..Settings::default() }.normalized();
        assert_eq!((s.small_file_mb, s.small_file_connections), (1, 1));
    }
}
