//! Persistence of `settings.json` and `downloads.json` (atomic writes).

use super::model::{Download, Status};
use super::segments;
use crate::settings::Settings;
use crate::util::write_atomic;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

pub const SETTINGS_FILE: &str = "settings.json";
pub const DOWNLOADS_FILE: &str = "downloads.json";

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct DownloadsFile {
    version: u32,
    downloads: Vec<Download>,
}

pub fn load_settings(dir: &Path) -> Settings {
    std::fs::read(dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
        .unwrap_or_default()
        .normalized()
}

pub fn save_settings(dir: &Path, s: &Settings) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
    write_atomic(&dir.join(SETTINGS_FILE), &json)
}

/// Loads the download list. A corrupt file is kept as `downloads.json.bak`
/// instead of being silently overwritten.
pub fn load_downloads(dir: &Path) -> Vec<Download> {
    let path = dir.join(DOWNLOADS_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<DownloadsFile>(&bytes) {
        Ok(f) => f.downloads,
        Err(e) => {
            eprintln!("downloads.json is unreadable ({e}); keeping a backup");
            let _ = std::fs::copy(&path, dir.join("downloads.json.bak"));
            Vec::new()
        }
    }
}

pub fn save_downloads(dir: &Path, downloads: &[Download]) -> io::Result<()> {
    let list: Vec<Download> = downloads
        .iter()
        .map(|d| {
            let mut d = d.clone();
            d.segments = segments::compact(&d.segments);
            d
        })
        .collect();
    let file = DownloadsFile {
        version: 1,
        downloads: list,
    };
    let json = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    write_atomic(&dir.join(DOWNLOADS_FILE), &json)
}

/// Items that were running when the app closed: `true` if they should be
/// resumed automatically, otherwise they become paused.
pub fn normalize_loaded(downloads: &mut [Download]) -> Vec<u64> {
    let mut was_active = Vec::new();
    for d in downloads.iter_mut() {
        if d.status.is_active() {
            was_active.push(d.id);
            d.status = Status::Paused;
        }
        if d.status == Status::Completed {
            d.segments.clear();
        }
        if let Some(t) = d.total_size
            && !d.segments.is_empty()
        {
            d.downloaded = segments::downloaded(&d.segments).min(t);
        }
    }
    was_active
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::Segment;

    #[test]
    fn roundtrip_and_normalize() {
        let dir = std::env::temp_dir().join(format!("zdm-store-{}", std::process::id()));
        let mut d = Download {
            id: 7,
            url: "https://example.com/a.zip".into(),
            status: Status::Downloading,
            total_size: Some(100),
            segments: vec![
                Segment { start: 0, end: 50, pos: 50 },
                Segment { start: 50, end: 100, pos: 60 },
            ],
            ..Default::default()
        };
        d.rt.speed = 123.0;
        save_downloads(&dir, &[d]).unwrap();
        let mut loaded = load_downloads(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].rt.speed, 0.0, "runtime state is not persisted");
        let active = normalize_loaded(&mut loaded);
        assert_eq!(active, vec![7]);
        assert_eq!(loaded[0].status, Status::Paused);
        assert_eq!(loaded[0].downloaded, 60);

        std::fs::write(dir.join(DOWNLOADS_FILE), b"{not json").unwrap();
        assert!(load_downloads(&dir).is_empty());
        assert!(dir.join("downloads.json.bak").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
