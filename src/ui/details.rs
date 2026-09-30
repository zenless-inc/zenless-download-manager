//! Bottom details panel for the focused download: info, segment map,
//! per-connection list and a speed sparkline.

use super::widgets::{category_badge, segment_map, sparkline, status_color, status_icon};
use super::{Action, App};
use crate::shared::kit;
use crate::shared::theme::{Palette, alpha, mix};
use egui_extras::{Size, StripBuilder};
use eframe::egui::{self, CornerRadius, FontId, RichText, Sense, Stroke, vec2};
use egui_phosphor::regular as icons;
use zenless_dm::engine::{ConnInfo, ConnState, Download, Status};

impl App {
    pub(super) fn details(&mut self, ui: &mut egui::Ui, p: &Palette, d: &Download) {
        // Header: badge, name, status, quick actions.
        ui.horizontal(|ui| {
            category_badge(ui, p, d.category, 30.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add(egui::Label::new(RichText::new(&d.file_name).size(15.5).strong().color(p.text)).truncate());
                let color = status_color(p, d.status);
                let mut line = format!("{} {}", status_icon(d.status), d.status.label());
                if d.status.is_active() && d.rt.speed > 0.0 {
                    line += &format!("  ·  {}  ·  {} left", kit::human_speed(d.rt.speed), d.rt.eta.map_or("—".into(), kit::human_eta));
                }
                ui.label(RichText::new(line).size(12.0).color(color));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("{}  Properties", icons::INFO)).clicked() {
                    self.actions.push(Action::Properties(d.id));
                }
                if ui.button(format!("{}  Folder", icons::FOLDER_OPEN)).clicked() {
                    self.actions.push(Action::OpenFolder(d.id));
                }
                if d.status == Status::Completed {
                    if kit::primary_button(ui, p, format!("{}  Open", icons::ARROW_SQUARE_OUT)).clicked() {
                        self.actions.push(Action::Open(d.id));
                    }
                } else if d.status.is_active() || d.status == Status::Queued {
                    if ui.button(format!("{}  Pause", icons::PAUSE)).clicked() {
                        self.actions.push(Action::Pause(vec![d.id]));
                    }
                } else if kit::primary_button(ui, p, format!("{}  Resume", icons::PLAY)).clicked() {
                    self.actions.push(Action::Resume(vec![d.id]));
                }
            });
        });
        ui.add_space(6.0);

        // Segment map.
        let seg_count = d.segments.len();
        ui.horizontal(|ui| {
            kit::section_label(ui, p, "Segment map");
            let extra = if d.status == Status::Completed {
                "complete".to_owned()
            } else if seg_count > 0 {
                format!("{seg_count} segments")
            } else if d.resumable == Some(false) {
                "single stream".to_owned()
            } else {
                String::new()
            };
            ui.label(RichText::new(extra).size(11.5).color(p.text_dim));
            if let Some(f) = d.progress() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.1}%", f * 100.0)).size(12.0).strong().color(p.text));
                });
            }
        });
        segment_map(ui, p, d, 16.0);
        ui.add_space(8.0);

        // Three columns: info | connections | speed graph.
        let h = ui.available_height().max(60.0);
        ui.spacing_mut().item_spacing.x = 18.0;
        StripBuilder::new(ui)
            .clip(true)
            .cell_layout(egui::Layout::top_down(egui::Align::Min))
            .size(Size::relative(0.40).at_least(250.0))
            .size(Size::relative(0.34).at_least(220.0))
            .size(Size::remainder().at_least(140.0))
            .horizontal(|mut strip| {
                strip.cell(|ui| self.info_grid(ui, p, d));
                strip.cell(|ui| connections(ui, p, d, h));
                strip.cell(|ui| {
                    ui.horizontal(|ui| {
                        kit::section_label(ui, p, "Speed · last 60 s");
                        let active: Vec<f32> = d.rt.history.iter().copied().filter(|&v| v > 0.0).collect();
                        if !active.is_empty() {
                            let avg = active.iter().sum::<f32>() / active.len() as f32;
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("avg {}", kit::human_speed(avg as f64))).size(11.0).color(p.text_dim));
                            });
                        }
                    });
                    let gh = (h - 26.0).clamp(40.0, 160.0);
                    let color = if d.status.is_active() { p.accent } else { p.text_dim };
                    let w = ui.available_width();
                    sparkline(ui, p, &d.rt.history, vec2(w, gh), color);
                });
            });
    }

    fn info_grid(&mut self, ui: &mut egui::Ui, p: &Palette, d: &Download) {
        egui::ScrollArea::vertical().id_salt("details_info").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("details_grid").striped(false)
                .num_columns(2)
                .spacing([12.0, 5.0])
                .min_col_width(70.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("URL").color(p.text_dim).size(12.5));
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new(icons::COPY).color(p.text_dim)).frame(false))
                            .on_hover_text("Copy URL")
                            .clicked()
                        {
                            ui.ctx().copy_text(d.url.clone());
                        }
                        ui.add(egui::Label::new(RichText::new(&d.url).size(12.5).color(p.text)).truncate())
                            .on_hover_text(&d.url);
                    });
                    ui.end_row();
                    let path = d.dir();
                    ui.label(RichText::new("Save to").color(p.text_dim).size(12.5));
                    ui.add(egui::Label::new(RichText::new(path.display().to_string()).size(12.5)).truncate())
                        .on_hover_text(d.path().display().to_string());
                    ui.end_row();
                    let size = match d.total_size {
                        Some(t) if d.status != Status::Completed => {
                            format!("{} of {}", kit::human_bytes(d.downloaded), kit::human_bytes(t))
                        }
                        Some(t) => kit::human_bytes(t),
                        None if d.downloaded > 0 => format!("{} (size unknown)", kit::human_bytes(d.downloaded)),
                        None => "Unknown".into(),
                    };
                    super::widgets::kv(ui, p, "Size", RichText::new(size).size(12.5));
                    let (text, color) = match d.resumable {
                        Some(true) => (format!("{}  Yes", icons::CHECK_CIRCLE), p.success),
                        Some(false) => (format!("{}  No — pausing restarts", icons::WARNING_CIRCLE), p.warning),
                        None => ("Not checked yet".into(), p.text_dim),
                    };
                    super::widgets::kv(ui, p, "Resumable", RichText::new(text).size(12.5).color(color));
                    let active = d.rt.conns.iter().filter(|c| c.state == ConnState::Receiving).count();
                    let conns = if d.status.is_active() {
                        format!("{active} active · {} max", d.connections)
                    } else {
                        format!("{} max", d.connections)
                    };
                    super::widgets::kv(ui, p, "Connections", RichText::new(conns).size(12.5));
                    if let Some(l) = d.speed_limit {
                        super::widgets::kv(ui, p, "Limit", RichText::new(kit::human_speed(l as f64)).size(12.5).color(p.warning));
                    }
                    if let Some(h) = &d.sha256 {
                        ui.label(RichText::new("SHA-256").color(p.text_dim).size(12.5));
                        ui.add(egui::Label::new(RichText::new(h).monospace().size(11.5)).truncate()).on_hover_text(h);
                        ui.end_row();
                    }
                    if let Some(e) = &d.error {
                        ui.label(RichText::new("Error").color(p.text_dim).size(12.5));
                        ui.add(egui::Label::new(RichText::new(e).size(12.5).color(p.danger)).wrap());
                        ui.end_row();
                    }
                });
            if let Some(note) = &d.note {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(alpha(p.warning, 0.10))
                    .stroke(Stroke::new(1.0, alpha(p.warning, 0.35)))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(format!("{}  {note}", icons::INFO)).size(12.0).color(p.text)).wrap());
                    });
            }
        });
    }
}

fn connections(ui: &mut egui::Ui, p: &Palette, d: &Download, h: f32) {
    let receiving = d.rt.conns.iter().filter(|c| c.state == ConnState::Receiving).count();
    ui.horizontal(|ui| {
        kit::section_label(ui, p, "Connections");
        if !d.rt.conns.is_empty() {
            ui.label(RichText::new(format!("{receiving}/{}", d.rt.conns.len())).size(11.5).color(p.text_dim));
        }
    });
    if d.rt.conns.is_empty() {
        let msg = match d.status {
            Status::Completed => "Finished — all connections closed.",
            Status::Queued => "Waiting in the queue.",
            Status::Paused => "Paused. Resume to reconnect.",
            Status::Failed => "Stopped after an error.",
            _ => "Connecting…",
        };
        ui.add_space(6.0);
        ui.label(RichText::new(msg).color(p.text_dim).size(12.5));
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("details_conns")
        .max_height((h - 22.0).max(40.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, c) in d.rt.conns.iter().enumerate() {
                connection_row(ui, p, c, i % 2 == 1);
            }
        });
}

/// One painted connection row: id, progress, range, speed, state.
fn connection_row(ui: &mut egui::Ui, p: &Palette, c: &ConnInfo, stripe: bool) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    let painter = ui.painter_at(rect);
    if stripe {
        painter.rect_filled(rect, CornerRadius::same(4), p.stripe);
    }
    let (state_color, bar_color) = match &c.state {
        ConnState::Receiving => (p.success, p.accent),
        ConnState::Connecting => (p.info, p.info),
        ConnState::Retrying { .. } => (p.warning, p.warning),
        ConnState::Failed(_) => (p.danger, p.danger),
        ConnState::Idle | ConnState::Waiting => (p.text_dim, mix(p.text_dim, p.bg, 0.3)),
    };
    let y = rect.center().y;
    let font = FontId::proportional(11.5);
    painter.text(egui::pos2(rect.left() + 6.0, y), egui::Align2::LEFT_CENTER, format!("#{}", c.id), font.clone(), p.text_dim);
    // Mini progress bar of the current range.
    let frac = if c.end > c.start { c.pos.saturating_sub(c.start) as f32 / (c.end - c.start) as f32 } else { 0.0 };
    let bar = egui::Rect::from_min_size(egui::pos2(rect.left() + 32.0, y - 2.5), vec2(40.0, 5.0));
    painter.rect_filled(bar, CornerRadius::same(2), mix(p.surface2, p.bg, 0.3));
    if frac > 0.0 {
        let mut fill = bar;
        fill.set_width((bar.width() * frac.clamp(0.0, 1.0)).max(3.0));
        painter.rect_filled(fill, CornerRadius::same(2), bar_color);
    }
    let range = if c.end > c.start {
        format!("{} – {}", kit::human_bytes(c.start), kit::human_bytes(c.end))
    } else {
        "—".to_owned()
    };
    painter.text(egui::pos2(rect.left() + 80.0, y), egui::Align2::LEFT_CENTER, range, font.clone(), p.text);
    // State dot + speed on the right.
    painter.circle_filled(egui::pos2(rect.right() - 9.0, y), 3.5, state_color);
    let speed = if c.speed >= 1.0 { kit::human_speed(c.speed) } else { c.state.label() };
    painter.text(egui::pos2(rect.right() - 20.0, y), egui::Align2::RIGHT_CENTER, speed, font, if c.speed >= 1.0 { p.text } else { state_color });
    let tip = match &c.state {
        ConnState::Retrying { error, .. } | ConnState::Failed(error) => format!("{} — {error}", c.state.label()),
        s => format!(
            "Connection #{} · {}\nReceived {} this session",
            c.id,
            s.label(),
            kit::human_bytes(c.received)
        ),
    };
    resp.on_hover_text(tip);
}
