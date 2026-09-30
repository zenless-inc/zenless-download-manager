//! Download-manager specific widgets: category badges, progress cells, the
//! IDM-style segment map and the speed sparkline. Everything is painted
//! from the active theme palette.

use crate::shared::kit;
use crate::shared::theme::{Palette, alpha, mix};
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use egui_phosphor::regular as icons;
use zenless_dm::category::Category;
use zenless_dm::engine::{ConnState, Download, HISTORY_LEN, Status};

pub fn status_color(p: &Palette, s: Status) -> Color32 {
    match s {
        Status::Downloading => p.accent,
        Status::Connecting => p.info,
        Status::Queued => p.accent2,
        Status::Paused => p.warning,
        Status::Completed => p.success,
        Status::Failed => p.danger,
    }
}

pub fn status_icon(s: Status) -> &'static str {
    match s {
        Status::Downloading => icons::ARROW_DOWN,
        Status::Connecting => icons::CIRCLE_NOTCH,
        Status::Queued => icons::CLOCK,
        Status::Paused => icons::PAUSE,
        Status::Completed => icons::CHECK_CIRCLE,
        Status::Failed => icons::WARNING_CIRCLE,
    }
}

pub fn category_icon(c: Category) -> &'static str {
    match c {
        Category::Video => icons::FILM_STRIP,
        Category::Music => icons::MUSIC_NOTES,
        Category::Documents => icons::FILE_TEXT,
        Category::Compressed => icons::FILE_ZIP,
        Category::Programs => icons::APP_WINDOW,
        Category::Images => icons::IMAGE,
        Category::Other => icons::FILE,
    }
}

pub fn category_color(p: &Palette, c: Category) -> Color32 {
    match c {
        Category::Video => mix(p.danger, p.warning, 0.35),
        Category::Music => p.accent2,
        Category::Documents => p.info,
        Category::Compressed => p.warning,
        Category::Programs => p.accent,
        Category::Images => p.success,
        Category::Other => p.text_dim,
    }
}

/// Rounded square with the category icon.
pub fn category_badge(ui: &mut egui::Ui, p: &Palette, c: Category, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let color = category_color(p, c);
    ui.painter()
        .rect_filled(rect, CornerRadius::same((size * 0.28) as u8), alpha(color, 0.16));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        category_icon(c),
        FontId::proportional(size * 0.6),
        color,
    );
    resp
}

/// Progress bar with a percentage label; animated stripes when the size is unknown.
pub fn progress_cell(ui: &mut egui::Ui, p: &Palette, d: &Download) {
    let color = status_color(p, d.status);
    let label_w = 42.0;
    let w = (ui.available_width() - label_w - 6.0).max(30.0);
    match d.progress() {
        Some(f) => {
            kit::progress_bar(ui, p, f, color, w, 6.0);
            ui.add_space(2.0);
            let text = if d.status == Status::Completed {
                "100%".to_owned()
            } else if f <= 0.0 {
                "0%".to_owned()
            } else if f < 0.1 {
                format!("{:.1}%", f * 100.0)
            } else {
                format!("{:.0}%", (f * 100.0).floor())
            };
            ui.label(egui::RichText::new(text).size(12.0).color(if d.status.is_active() { p.text } else { p.text_dim }));
        }
        None => {
            indeterminate_bar(ui, p, color, w, 6.0, d.status.is_active());
            ui.add_space(2.0);
            ui.label(egui::RichText::new(kit::human_bytes(d.downloaded)).size(12.0).color(p.text_dim));
        }
    }
}

/// Moving stripes for transfers of unknown size.
pub fn indeterminate_bar(ui: &mut egui::Ui, p: &Palette, color: Color32, width: f32, height: f32, animate: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let r = CornerRadius::same((height / 2.0) as u8);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, r, mix(p.surface2, p.bg, 0.3));
    if !animate {
        return;
    }
    let t = ui.input(|i| i.time) as f32;
    let seg = (rect.width() * 0.3).max(20.0);
    let x = rect.left() + ((t * 0.6).fract() * (rect.width() + seg)) - seg;
    let bar = Rect::from_min_max(pos2(x.max(rect.left()), rect.top()), pos2((x + seg).min(rect.right()), rect.bottom()));
    if bar.width() > 0.0 {
        painter.rect_filled(bar, r, color);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// IDM-style segment map: one bar for the file, each segment's downloaded
/// part filled (alternating shades), live connection heads highlighted.
pub fn segment_map(ui: &mut egui::Ui, p: &Palette, d: &Download, height: f32) -> Response {
    let width = ui.available_width().max(40.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    let radius = CornerRadius::same(4);
    painter.rect_filled(rect, radius, p.input);
    let inner = rect.shrink(2.0);
    let done_color = status_color(p, d.status);
    let alt_color = mix(done_color, p.accent2, 0.35);

    let total = d.total_size.unwrap_or(0);
    if d.status == Status::Completed {
        painter.rect_filled(inner, CornerRadius::same(3), alpha(p.success, 0.85));
    } else if total > 0 && !d.segments.is_empty() {
        let x_of = |b: u64| inner.left() + (b as f64 / total as f64 * inner.width() as f64) as f32;
        let mut segs = d.segments.clone();
        segs.sort_by_key(|s| s.start);
        for (i, s) in segs.iter().enumerate() {
            let x0 = x_of(s.start);
            let x1 = x_of(s.pos).max(x0 + if s.pos > s.start { 1.0 } else { 0.0 });
            if x1 > x0 {
                let c = if i % 2 == 0 { done_color } else { alt_color };
                painter.rect_filled(
                    Rect::from_min_max(pos2(x0, inner.top()), pos2(x1, inner.bottom())),
                    CornerRadius::ZERO,
                    c,
                );
            }
            if s.start > 0 {
                painter.vline(x0, inner.y_range(), Stroke::new(1.0, alpha(p.bg, 0.8)));
            }
        }
        for c in &d.rt.conns {
            if c.state == ConnState::Receiving && c.end > c.start {
                let x = x_of(c.pos);
                painter.vline(x, rect.y_range(), Stroke::new(2.0, p.text));
            }
        }
    } else if total > 0 && d.downloaded > 0 {
        let f = (d.downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32;
        let mut r = inner;
        r.set_width(inner.width() * f);
        painter.rect_filled(r, CornerRadius::same(3), done_color);
    } else if d.status.is_active() {
        let t = ui.input(|i| i.time) as f32;
        let w = inner.width() * 0.25;
        let x = inner.left() + (t * 0.5).fract() * (inner.width() + w) - w;
        let bar = Rect::from_min_max(pos2(x.max(inner.left()), inner.top()), pos2((x + w).min(inner.right()), inner.bottom()));
        painter.rect_filled(bar, CornerRadius::same(3), alpha(done_color, 0.8));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    painter.rect_stroke(rect, radius, Stroke::new(1.0, p.border), StrokeKind::Inside);

    // Tooltip: the segment under the pointer.
    if total > 0
        && let Some(pos) = resp.hover_pos()
    {
        let byte = ((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0) as f64 * total as f64;
        let byte = byte as u64;
        if let Some((i, s)) = d
            .segments
            .iter()
            .enumerate()
            .find(|(_, s)| s.start <= byte && byte < s.end.max(s.start + 1))
        {
            let pct = if s.end > s.start { s.downloaded() as f64 / (s.end - s.start) as f64 * 100.0 } else { 100.0 };
            resp.clone().on_hover_text(format!(
                "Segment {} · {} – {}\n{:.0}% downloaded",
                i + 1,
                kit::human_bytes(s.start),
                kit::human_bytes(s.end),
                pct
            ));
        }
    }
    resp
}

/// Area chart of speed samples (bytes/s), right-aligned so a partial
/// history grows from the right edge.
pub fn sparkline(ui: &mut egui::Ui, p: &Palette, samples: &[f32], size: Vec2, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    // Baseline + a faint mid grid line.
    painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.border));
    painter.hline(rect.x_range(), rect.center().y, Stroke::new(1.0, alpha(p.border, 0.35)));
    let peak = samples.iter().copied().fold(0.0f32, f32::max);
    if samples.len() < 2 || peak <= 0.0 {
        return resp;
    }
    let max = peak.max(1.0) * 1.15;
    // Keep the plot below the peak label drawn in the top-left corner.
    let label_room = 14.0_f32.min(rect.height() * 0.4);
    let plot_height = rect.height() - 3.0 - label_room;
    let step = rect.width() / (HISTORY_LEN.max(2) - 1) as f32;
    let n = samples.len().min(HISTORY_LEN);
    let samples = &samples[samples.len() - n..];
    let pts: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let x = rect.right() - (n - 1 - i) as f32 * step;
            let y = rect.bottom() - 1.0 - (v / max).clamp(0.0, 1.0) * plot_height;
            pos2(x, y)
        })
        .collect();
    let mut mesh = egui::Mesh::default();
    for (i, pt) in pts.iter().enumerate() {
        mesh.colored_vertex(*pt, alpha(color, 0.32));
        mesh.colored_vertex(pos2(pt.x, rect.bottom()), alpha(color, 0.02));
        if i > 0 {
            let b = (i as u32) * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b, b + 1);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.6, color)));
    if let Some(last) = pts.last() {
        painter.circle_filled(*last, 2.5, color);
    }
    painter.text(
        rect.left_top() + vec2(2.0, 1.0),
        egui::Align2::LEFT_TOP,
        kit::human_speed(peak as f64),
        FontId::proportional(10.5),
        alpha(p.text_dim, 0.8),
    );
    resp
}

/// "5 min ago" / "Yesterday" / date.
pub fn relative_time(unix: u64) -> String {
    let now = zenless_dm::util::unix_now();
    let d = now.saturating_sub(unix);
    match d {
        0..=59 => "Just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        86_400..=172_799 => "Yesterday".into(),
        _ if d < 7 * 86_400 => format!("{} days ago", d / 86_400),
        _ => kit::human_date(unix).chars().take(10).collect(),
    }
}

/// Dim label + value row for detail grids.
pub fn kv(ui: &mut egui::Ui, p: &Palette, key: &str, value: impl Into<egui::WidgetText>) {
    ui.label(egui::RichText::new(key).color(p.text_dim).size(12.5));
    ui.add(egui::Label::new(value).truncate());
    ui.end_row();
}

/// Small colored dot (status indicators).
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 7.0, alpha(color, 0.18));
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Red "danger" filled button.
pub fn danger_button(ui: &mut egui::Ui, p: &Palette, text: impl Into<String>) -> Response {
    let fg = if luminance(p.danger) > 0.55 { Color32::BLACK } else { Color32::WHITE };
    ui.add(
        egui::Button::new(egui::RichText::new(text.into()).color(fg).strong())
            .fill(p.danger)
            .stroke(Stroke::NONE),
    )
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}
