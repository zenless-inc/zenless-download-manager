//! Zenless shared UI kit: small widgets and formatters used by every Zenless app.
//!
//! This file is shared verbatim between Zenless Download Manager, Zenless Torrent
//! and the Zenless Installer. Keep the copies in sync.

use super::theme::{Palette, alpha, mix};
use eframe::egui::{self, Color32, CornerRadius, FontId, Response, RichText, Sense, Stroke, Vec2};

/// Filled accent-colored button (primary action).
pub fn primary_button(ui: &mut egui::Ui, p: &Palette, text: impl Into<String>) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text.into()).color(p.accent_fg).strong())
            .fill(p.accent)
            .stroke(Stroke::NONE),
    )
}

/// Button with a Phosphor icon and a label, e.g. `icon_button(ui, regular::PLUS, "Add")`.
pub fn icon_button(ui: &mut egui::Ui, icon: &str, text: &str) -> Response {
    ui.add(egui::Button::new(format!("{icon}  {text}")))
}

/// Borderless toolbar button: large icon above a small caption.
pub fn toolbar_button(ui: &mut egui::Ui, p: &Palette, icon: &str, text: &str, enabled: bool) -> Response {
    let size = Vec2::new(64.0, 50.0);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    if enabled && (response.hovered() || response.has_focus()) {
        let fill = if response.is_pointer_button_down_on() {
            mix(p.surface2, p.accent, 0.3)
        } else {
            mix(p.surface2, p.accent, 0.12)
        };
        painter.rect_filled(rect, ui.visuals().widgets.hovered.corner_radius, fill);
    }
    let color = if enabled { p.text } else { alpha(p.text_dim, 0.5) };
    let icon_color = if enabled { p.accent } else { alpha(p.text_dim, 0.5) };
    painter.text(
        rect.center_top() + Vec2::new(0.0, 7.0),
        egui::Align2::CENTER_TOP,
        icon,
        FontId::proportional(20.0),
        icon_color,
    );
    painter.text(
        rect.center_bottom() - Vec2::new(0.0, 6.0),
        egui::Align2::CENTER_BOTTOM,
        text,
        FontId::proportional(11.5),
        color,
    );
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Thin rounded progress bar. `fraction` is clamped to 0..=1.
pub fn progress_bar(ui: &mut egui::Ui, p: &Palette, fraction: f32, color: Color32, width: f32, height: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let r = CornerRadius::same((height / 2.0) as u8);
    let painter = ui.painter();
    painter.rect_filled(rect, r, mix(p.surface2, p.bg, 0.3));
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut fill = rect;
        fill.set_width((rect.width() * f).max(height));
        painter.rect_filled(fill, r, color);
    }
    response
}

/// Small rounded status label, e.g. "Downloading".
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let pad = Vec2::new(8.0, 3.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(255), alpha(color, 0.16));
    ui.painter().galley(rect.min + pad, galley, color);
    response
}

/// A padded card frame using the theme's surface color.
pub fn card(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
}

/// Section title in the dim text color.
pub fn section_label(ui: &mut egui::Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).small().strong().color(p.text_dim));
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// `1536` → `"1.5 KB"`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// Bytes per second → `"2.4 MB/s"`.
pub fn human_speed(bps: f64) -> String {
    if bps < 1.0 {
        "—".to_owned()
    } else {
        format!("{}/s", human_bytes(bps as u64))
    }
}

/// Seconds → `"1h 04m"`, `"3m 12s"`, `"8s"`.
pub fn human_eta(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "—".to_owned();
    }
    let s = secs.round() as u64;
    let (d, h, m, s) = (s / 86_400, (s / 3600) % 24, (s / 60) % 60, s % 60);
    if d > 0 {
        format!("{d}d {h:02}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Unix seconds → local-ish `"YYYY-MM-DD HH:MM"` (UTC, no tz database needed).
pub fn human_date(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, (secs / 60) % 60)
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Auto screenshot (debug aid)
// ---------------------------------------------------------------------------

/// When the `ZENLESS_SCREENSHOT` environment variable holds a file path, the app
/// saves a PNG of its main window after `ZENLESS_SCREENSHOT_FRAMES` frames
/// (default 40) and then closes. Call [`AutoScreenshot::tick`] once per frame.
pub struct AutoScreenshot {
    path: Option<std::path::PathBuf>,
    frames_left: u32,
    requested: bool,
}

impl AutoScreenshot {
    pub fn from_env() -> Self {
        let path = std::env::var_os("ZENLESS_SCREENSHOT").map(std::path::PathBuf::from);
        let frames_left = std::env::var("ZENLESS_SCREENSHOT_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40);
        Self {
            path,
            frames_left,
            requested: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.path.is_some()
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        let Some(path) = self.path.clone() else { return };
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Err(e) = save_png(&path, w as u32, h as u32, &rgba) {
                eprintln!("screenshot failed: {e}");
            }
            self.path = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.frames_left > 0 {
            self.frames_left -= 1;
        } else if !self.requested {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint();
    }
}

/// Writes an RGBA8 image as an (uncompressed) PNG. No external dependencies.
pub fn save_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }

    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in rgba.chunks(stride).take(height as usize) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    // zlib stream made of stored (uncompressed) deflate blocks
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1536), "1.50 KB");
        assert_eq!(human_bytes(10 * 1024 * 1024), "10.0 MB");
    }

    #[test]
    fn eta() {
        assert_eq!(human_eta(8.0), "8s");
        assert_eq!(human_eta(192.0), "3m 12s");
        assert_eq!(human_eta(3840.0), "1h 04m");
    }

    #[test]
    fn date() {
        assert_eq!(human_date(0), "1970-01-01 00:00");
        assert_eq!(human_date(1_700_000_000), "2023-11-14 22:13");
    }
}
