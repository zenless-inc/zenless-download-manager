//! Left sidebar: brand, status filters, categories and a speed summary.

use super::widgets::{category_color, category_icon, sparkline};
use super::{App, Filter};
use crate::shared::kit;
use crate::shared::theme::{Palette, alpha, mix};
use eframe::egui::{self, Color32, CornerRadius, FontId, RichText, Sense, Stroke, vec2};
use egui_phosphor::regular as icons;
use zenless_dm::category::Category;

impl App {
    pub(super) fn sidebar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        brand(ui, p);
        ui.add_space(12.0);

        // Speed summary pinned to the bottom; the lists scroll above it.
        egui::Panel::bottom("sidebar_summary")
            .frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 0 }))
            .show_separator_line(false)
            .resizable(false)
            .show(ui, |ui| self.speed_summary(ui, p));
        egui::ScrollArea::vertical()
            .id_salt("sidebar_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| self.sidebar_lists(ui, p));
    }

    fn sidebar_lists(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let snap = self.snap.clone();
        ui.spacing_mut().item_spacing.y = 2.0;
        kit::section_label(ui, p, "Downloads");
        ui.add_space(2.0);
        let status_items: [(Filter, &str, &str, Color32); 6] = [
            (Filter::All, icons::SQUARES_FOUR, "All downloads", p.text),
            (Filter::Downloading, icons::ARROW_DOWN, "Downloading", p.accent),
            (Filter::Queued, icons::CLOCK, "Queued", p.accent2),
            (Filter::Paused, icons::PAUSE, "Paused", p.warning),
            (Filter::Completed, icons::CHECK_CIRCLE, "Completed", p.success),
            (Filter::Failed, icons::WARNING_CIRCLE, "Failed", p.danger),
        ];
        for (filter, icon, label, color) in status_items {
            let count = snap.count(|d| filter.matches(d));
            if nav_item(ui, p, icon, label, color, count, self.filter == filter).clicked() {
                self.filter = filter;
            }
        }

        ui.add_space(12.0);
        kit::section_label(ui, p, "Categories");
        ui.add_space(2.0);
        for c in Category::ALL {
            let filter = Filter::Category(c);
            let count = snap.count(|d| d.category == c);
            if nav_item(ui, p, category_icon(c), c.label(), category_color(p, c), count, self.filter == filter).clicked() {
                self.filter = filter;
            }
        }
    }

    fn speed_summary(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let snap = self.snap.clone();
        let active = snap.count(|d| d.status.is_active());
        egui::Frame::new()
            .fill(mix(p.surface, p.bg, 0.45))
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::ARROW_DOWN).color(p.accent).size(16.0));
                    let speed = if snap.total_speed >= 1.0 { kit::human_speed(snap.total_speed) } else { "0 B/s".into() };
                    ui.label(RichText::new(speed).size(17.0).strong().color(p.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{active} active")).size(11.5).color(p.text_dim));
                    });
                });
                let w = ui.available_width();
                sparkline(ui, p, &snap.speed_history, vec2(w, 38.0), p.accent);
                ui.add_space(2.0);
                let limit = if self.settings.speed_limit_enabled {
                    format!("{}  Limited to {}", icons::GAUGE, kit::human_speed(self.settings.speed_limit_kib as f64 * 1024.0))
                } else {
                    format!("{}  No speed limit", icons::GAUGE)
                };
                ui.label(RichText::new(limit).size(12.0).color(if self.settings.speed_limit_enabled {
                    p.warning
                } else {
                    p.text_dim
                }));
            });
    }
}

fn brand(ui: &mut egui::Ui, p: &Palette) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(9), p.accent);
        painter.text(
            rect.center() + vec2(0.0, 1.0),
            egui::Align2::CENTER_CENTER,
            icons::DOWNLOAD_SIMPLE,
            FontId::proportional(19.0),
            p.accent_fg,
        );
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new("Zenless").size(16.0).strong().color(p.text));
            ui.label(RichText::new("Download Manager").size(11.5).color(p.text_dim));
        });
    });
}

/// A sidebar row: icon, label and a count badge.
fn nav_item(ui: &mut egui::Ui, p: &Palette, icon: &str, label: &str, color: Color32, count: usize, selected: bool) -> egui::Response {
    let height = 26.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let painter = ui.painter();
    let r = CornerRadius::same(7);
    if selected {
        painter.rect_filled(rect, r, mix(p.surface, p.accent, 0.13));
        let bar = egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 6.0), vec2(3.0, height - 12.0));
        painter.rect_filled(bar, CornerRadius::same(2), p.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect, r, mix(p.surface, p.text, 0.05));
    }
    painter.text(
        rect.left_center() + vec2(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        FontId::proportional(16.0),
        if selected { p.accent } else { color },
    );
    painter.text(
        rect.left_center() + vec2(38.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        if selected { p.text } else { mix(p.text, p.text_dim, 0.25) },
    );
    if count > 0 {
        let text = count.to_string();
        let galley = painter.layout_no_wrap(text, FontId::proportional(11.5), if selected { p.text } else { p.text_dim });
        let pad = vec2(7.0, 2.0);
        let size = galley.size() + pad * 2.0;
        let badge = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 8.0 - size.x.max(22.0), rect.center().y - size.y / 2.0),
            vec2(size.x.max(22.0), size.y),
        );
        painter.rect_filled(badge, CornerRadius::same(255), if selected { alpha(p.accent, 0.22) } else { p.surface2 });
        painter.galley(badge.center() - galley.size() / 2.0, galley, p.text);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
