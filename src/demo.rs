//! `ZENLESS_DEMO=1`: a realistic, animated but completely fake download list
//! for screenshots and UI work. Demo mode never touches the network, never
//! writes files and never saves `downloads.json`.

use crate::category::Category;
use crate::engine::model::{ConnInfo, ConnState, Download, Id, RequestInfo, Segment, Status};
use crate::engine::segments::{self, Slot};
use std::collections::HashMap;
use std::path::Path;

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

struct Item {
    name: &'static str,
    host: &'static str,
    size: u64,
    status: Status,
    progress: f64,
    connections: u32,
    /// Base speed in bytes/s while downloading.
    speed: f64,
    age_mins: u64,
    error: Option<&'static str>,
}

const ITEMS: &[Item] = &[
    Item { name: "ubuntu-24.04.3-desktop-amd64.iso", host: "releases.ubuntu.com", size: 6_046 * MB, status: Status::Downloading, progress: 0.47, connections: 8, speed: 11.8 * MB as f64, age_mins: 14, error: None },
    Item { name: "Blender-4.5.2-windows-x64.msi", host: "download.blender.org", size: 356 * MB, status: Status::Downloading, progress: 0.81, connections: 16, speed: 6.4 * MB as f64, age_mins: 6, error: None },
    Item { name: "Northern Lights 4K HDR (2025).mkv", host: "cdn.example-media.net", size: 14 * GB + 210 * MB, status: Status::Downloading, progress: 0.12, connections: 8, speed: 17.5 * MB as f64, age_mins: 3, error: None },
    Item { name: "Lo-fi Beats Vol. 3 (FLAC).zip", host: "files.example-music.com", size: 612 * MB, status: Status::Queued, progress: 0.0, connections: 8, speed: 0.0, age_mins: 2, error: None },
    Item { name: "podcast-episode-142.mp3", host: "media.example-pod.fm", size: 88 * MB, status: Status::Queued, progress: 0.0, connections: 4, speed: 0.0, age_mins: 1, error: None },
    Item { name: "aurora-wallpaper-pack.zip", host: "static.example-art.io", size: 1_240 * MB, status: Status::Paused, progress: 0.63, connections: 8, speed: 0.0, age_mins: 95, error: None },
    Item { name: "climate-dataset-2026-q3.tar.gz", host: "data.example-science.org", size: 22 * GB, status: Status::Failed, progress: 0.34, connections: 12, speed: 0.0, age_mins: 240, error: Some("HTTP 403 Forbidden (the link may have expired or needs a login)") },
    Item { name: "The Rust Programming Language.pdf", host: "books.example.dev", size: 8 * MB + 420 * 1024, status: Status::Completed, progress: 1.0, connections: 8, speed: 0.0, age_mins: 60 * 20, error: None },
    Item { name: "node-v24.9.0-x64.msi", host: "nodejs.org", size: 31 * MB, status: Status::Completed, progress: 1.0, connections: 8, speed: 0.0, age_mins: 60 * 26, error: None },
    Item { name: "IMG_20260914_183022.heic", host: "photos.example-cloud.com", size: 4 * MB + 120 * 1024, status: Status::Completed, progress: 1.0, connections: 1, speed: 0.0, age_mins: 60 * 30, error: None },
    Item { name: "VSCodeUserSetup-x64-1.105.0.exe", host: "update.code.example.com", size: 104 * MB, status: Status::Completed, progress: 1.0, connections: 8, speed: 0.0, age_mins: 60 * 49, error: None },
    Item { name: "Quarterly Report Q3.docx", host: "share.example-corp.com", size: 2 * MB + 310 * 1024, status: Status::Completed, progress: 1.0, connections: 1, speed: 0.0, age_mins: 60 * 72, error: None },
    Item { name: "router-firmware-v3.2.bin", host: "support.example-net.com", size: 64 * MB, status: Status::Completed, progress: 1.0, connections: 8, speed: 0.0, age_mins: 60 * 100, error: None },
];

/// Deterministic pseudo-noise in -1..1.
fn noise(x: f64) -> f64 {
    (x * 1.7).sin() * 0.5 + (x * 0.63 + 1.3).sin() * 0.3 + (x * 3.1 + 0.4).sin() * 0.2
}

/// Builds segments for a partially finished demo download (dynamic
/// segmentation already happened a few times, so they look organic).
fn demo_segments(size: u64, progress: f64, conns: u32, seed: f64) -> Vec<Segment> {
    let mut segs = segments::initial_segments(size, conns);
    for (i, s) in segs.iter_mut().enumerate() {
        let f = (progress + noise(seed + i as f64 * 2.3) * 0.18).clamp(0.02, 1.0);
        s.pos = s.start + ((s.end - s.start) as f64 * f) as u64;
    }
    segs
}

/// Fake download list rooted at `dir`.
pub fn sample_downloads(dir: &Path) -> Vec<Download> {
    let now = crate::util::unix_now();
    ITEMS
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let id = i as Id + 1;
            let segs = match it.status {
                Status::Completed | Status::Queued => Vec::new(),
                _ => demo_segments(it.size, it.progress, it.connections, i as f64 * 5.1),
            };
            let downloaded = if it.status == Status::Completed { it.size } else { segments::downloaded(&segs) };
            let history: Vec<f32> = if it.speed > 0.0 {
                (0..60)
                    .map(|t| (it.speed * (1.0 + 0.22 * noise(t as f64 * 0.37 + i as f64))) as f32)
                    .collect()
            } else if it.status == Status::Paused {
                (0..60)
                    .map(|t| if t < 40 { (4.2 * MB as f64 * (1.0 + 0.2 * noise(t as f64 * 0.4))) as f32 } else { 0.0 })
                    .collect()
            } else {
                Vec::new()
            };
            let mut d = Download {
                id,
                url: format!("https://{}/downloads/{}", it.host, it.name.replace(' ', "%20")),
                file_name: it.name.to_owned(),
                save_dir: dir.to_path_buf(),
                target_dir: Some(dir.to_path_buf()),
                total_size: Some(it.size),
                downloaded,
                resumable: Some(it.connections > 1),
                status: it.status,
                error: it.error.map(str::to_owned),
                category: Category::from_file_name(it.name),
                connections: it.connections,
                segments: segs,
                added_at: now.saturating_sub(it.age_mins * 60),
                completed_at: (it.status == Status::Completed).then(|| now.saturating_sub(it.age_mins * 60 - 120)),
                request: RequestInfo {
                    referrer: Some(format!("https://{}/", it.host)),
                    ..Default::default()
                },
                source: Some(if i % 2 == 0 { "chrome" } else { "firefox" }.to_owned()),
                ..Default::default()
            };
            if it.connections == 1 && it.status != Status::Completed {
                d.note = Some("This server does not support resuming.".into());
            }
            d.rt.history = history;
            d.rt.speed = it.speed;
            if i == 7 {
                d.sha256 = Some("9f2c4b1e7d0a3c58e6b4f1a2d9c7e05b3a8f6d4c2e1b0a9f8e7d6c5b4a392817".into());
            }
            d
        })
        .collect()
}

/// Animates demo downloads: connections advance, finish segments and
/// split the largest remaining one, just like the real engine.
pub struct DemoSim {
    slots: HashMap<Id, Vec<Slot>>,
    owners: HashMap<Id, Vec<Option<usize>>>,
    base_speed: HashMap<Id, f64>,
    acc: HashMap<Id, (f64, f64)>,
    t: f64,
}

impl DemoSim {
    pub fn new(downloads: &[Download]) -> Self {
        let mut sim = DemoSim {
            slots: HashMap::new(),
            owners: HashMap::new(),
            base_speed: HashMap::new(),
            acc: HashMap::new(),
            t: 0.0,
        };
        for d in downloads {
            sim.base_speed.insert(d.id, if d.rt.speed > 0.0 { d.rt.speed } else { 5.0 * MB as f64 });
        }
        sim
    }

    /// Advances all demo downloads by `dt` seconds; returns bytes "received".
    pub fn tick(&mut self, downloads: &mut [Download], dt: f64) -> u64 {
        self.t += dt;
        let mut total = 0u64;
        for d in downloads.iter_mut() {
            if d.status != Status::Downloading {
                d.rt.conns.clear();
                continue;
            }
            let Some(size) = d.total_size else { continue };
            let slots = self.slots.entry(d.id).or_insert_with(|| {
                let segs = if d.segments.is_empty() {
                    segments::initial_segments(size, d.connections)
                } else {
                    d.segments.clone()
                };
                segs.into_iter().map(|seg| Slot { seg, active: false }).collect()
            });
            let owners = self.owners.entry(d.id).or_insert_with(|| vec![None; d.connections as usize]);
            let base = *self.base_speed.entry(d.id).or_insert(5.0 * MB as f64);
            let n = owners.len().max(1) as f64;
            let mut bytes = 0u64;
            let mut conns = Vec::with_capacity(owners.len());
            for (c, owner) in owners.iter_mut().enumerate() {
                if owner.is_none() {
                    *owner = segments::claim(slots);
                }
                let Some(i) = *owner else {
                    conns.push(ConnInfo { id: c + 1, start: 0, end: 0, pos: 0, speed: 0.0, received: 0, state: ConnState::Idle });
                    continue;
                };
                let speed = (base / n) * (1.0 + 0.35 * noise(self.t * 0.8 + c as f64 * 1.9 + d.id as f64));
                let step = ((speed * dt) as u64).min(slots[i].seg.remaining());
                slots[i].seg.pos += step;
                bytes += step;
                let seg = slots[i].seg;
                conns.push(ConnInfo {
                    id: c + 1,
                    start: seg.start,
                    end: seg.end,
                    pos: seg.pos,
                    speed,
                    received: seg.downloaded(),
                    state: ConnState::Receiving,
                });
                if seg.is_done() {
                    slots[i].active = false;
                    *owner = None;
                }
            }
            d.segments = slots.iter().map(|s| s.seg).collect();
            d.downloaded = segments::downloaded(&d.segments);
            d.rt.conns = conns;
            let inst = bytes as f64 / dt.max(0.001);
            d.rt.speed = d.rt.speed * 0.75 + inst * 0.25;
            d.rt.eta = d.remaining().filter(|_| d.rt.speed > 1.0).map(|r| r as f64 / d.rt.speed);
            let acc = self.acc.entry(d.id).or_insert((0.0, 0.0));
            acc.0 += bytes as f64;
            acc.1 += dt;
            if acc.1 >= 1.0 {
                d.rt.history.push((acc.0 / acc.1) as f32);
                if d.rt.history.len() > crate::engine::HISTORY_LEN {
                    d.rt.history.remove(0);
                }
                *acc = (0.0, 0.0);
            }
            if segments::all_done(&d.segments) {
                d.status = Status::Completed;
                d.completed_at = Some(crate::util::unix_now());
                d.segments.clear();
                d.rt.conns.clear();
                d.rt.speed = 0.0;
            }
            total += bytes;
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_items_are_consistent() {
        let items = sample_downloads(Path::new("C:/Demo"));
        assert!(items.len() >= 10);
        for d in &items {
            if !d.segments.is_empty() {
                assert!(segments::validate(&d.segments, d.total_size.unwrap()), "{}", d.file_name);
            }
        }
        let mut items = items;
        let mut sim = DemoSim::new(&items);
        let before: u64 = items.iter().map(|d| d.downloaded).sum();
        let bytes = sim.tick(&mut items, 0.25);
        assert!(bytes > 0);
        let after: u64 = items.iter().map(|d| d.downloaded).sum();
        assert_eq!(after - before, bytes);
    }
}
