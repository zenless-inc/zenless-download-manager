//! Top toolbar and bottom status bar.

use super::widgets::dot;
use super::{Action, App};
use crate::shared::kit;
use crate::shared::theme::{Palette, alpha, mix};
use eframe::egui::{self, RichText, Stroke, vec2};
use egui_phosphor::regular as icons;
use zenless_dm::engine::{Command, Status};

impl App {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let sel = self.selected_downloads();
        let can_resume = sel.iter().any(|d| d.status.can_resume());
        let can_pause = sel.iter().any(|d| d.status.is_active() || d.status == Status::Queued);
        let any_running = self
            .snap
            .downloads
            .iter()
            .any(|d| d.status.is_active() || d.status == Status::Queued);
        let queue_running = self.snap.queue_running;
        let folder_target = sel.first().map(|d| d.path());

        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if kit::toolbar_button(ui, p, icons::PLUS, "Add URL", true)
                .on_hover_text("New download (Ctrl+N)")
                .clicked()
            {
                self.actions.push(Action::NewDownload(None));
            }
            if kit::toolbar_button(ui, p, icons::PLAY, "Resume", can_resume)
                .on_hover_text("Resume the selected downloads")
                .clicked()
            {
                self.actions.push(Action::Resume(self.selected_ids()));
            }
            if kit::toolbar_button(ui, p, icons::PAUSE, "Pause", can_pause)
                .on_hover_text("Pause the selected downloads (Space)")
                .clicked()
            {
                self.actions.push(Action::Pause(self.selected_ids()));
            }
            if kit::toolbar_button(ui, p, icons::PAUSE_CIRCLE, "Pause all", any_running)
                .on_hover_text("Pause every running and queued download")
                .clicked()
            {
                self.actions.push(Action::PauseAll);
            }
            if kit::toolbar_button(ui, p, icons::TRASH, "Delete", !sel.is_empty())
                .on_hover_text("Remove the selected downloads (Del)")
                .clicked()
            {
                self.actions.push(Action::AskDelete(self.selected_ids()));
            }
            toolbar_separator(ui, p);
            let (icon, label, tip) = if queue_running {
                (icons::STOP_CIRCLE, "Stop queue", "Stop starting queued downloads and send queue downloads back to the queue")
            } else {
                (icons::PLAY_CIRCLE, "Start queue", "Start processing the download queue")
            };
            if kit::toolbar_button(ui, p, icon, label, true).on_hover_text(tip).clicked() {
                self.engine.send(if queue_running { Command::StopQueue } else { Command::StartQueue });
            }
            if kit::toolbar_button(ui, p, icons::FOLDER_OPEN, "Open folder", true)
                .on_hover_text("Show the selected file, or the download folder")
                .clicked()
            {
                match folder_target {
                    Some(path) => self.reveal(&path),
                    None => {
                        let dir = self.settings.download_dir.clone();
                        self.open_path(&dir);
                    }
                }
            }
            if kit::toolbar_button(ui, p, icons::GEAR_SIX, "Settings", true).clicked() {
                self.actions.push(Action::OpenSettings);
            }

            // Search box on the right.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let width = (ui.available_width() - 8.0).clamp(120.0, 280.0);
                egui::Frame::new()
                    .fill(p.input)
                    .stroke(Stroke::new(
                        1.0,
                        if self.search.is_empty() { p.border } else { mix(p.border, p.accent, 0.6) },
                    ))
                    .corner_radius(ui.visuals().widgets.inactive.corner_radius)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icons::MAGNIFYING_GLASS).color(p.text_dim));
                            let clear_w = if self.search.is_empty() { 0.0 } else { 18.0 };
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.search)
                                    .hint_text("Search downloads")
                                    .frame(egui::Frame::NONE)
                                    .desired_width(ui.available_width() - clear_w),
                            );
                            self.search_focused = resp.has_focus();
                            if !self.search.is_empty()
                                && ui
                                    .add(egui::Button::new(RichText::new(icons::X).color(p.text_dim)).frame(false))
                                    .on_hover_text("Clear search")
                                    .clicked()
                            {
                                self.search.clear();
                            }
                        });
                    });
            });
        });
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let snap = self.snap.clone();
        let active = snap.count(|d| d.status.is_active());
        let queued = snap.count(|d| d.status == Status::Queued);
        let done = snap.count(|d| d.status == Status::Completed);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.label(
                RichText::new(format!("{}  {}", icons::ARROW_DOWN, speed_or_idle(snap.total_speed)))
                    .color(if snap.total_speed > 0.0 { p.accent } else { p.text_dim })
                    .strong(),
            );
            ui.label(RichText::new(format!("{active} active")).color(p.text_dim));
            ui.label(RichText::new(format!("{queued} queued")).color(p.text_dim));
            ui.label(RichText::new(format!("{done} completed")).color(p.text_dim));
            if !snap.queue_running {
                ui.label(RichText::new(format!("{}  Queue stopped", icons::STOP_CIRCLE)).color(p.warning));
            }
            if self.demo {
                ui.label(RichText::new("DEMO MODE · sample data").color(p.accent2).strong());
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                // Quick speed-limit toggle (+ presets on right click).
                let on = self.settings.speed_limit_enabled;
                let text = if on {
                    format!("{}  Limit {}", icons::GAUGE, kit::human_speed(self.settings.speed_limit_kib as f64 * 1024.0))
                } else {
                    format!("{}  No speed limit", icons::GAUGE)
                };
                let btn = egui::Button::new(RichText::new(text).size(12.5).color(if on { p.warning } else { p.text_dim }))
                    .fill(if on { alpha(p.warning, 0.12) } else { egui::Color32::TRANSPARENT })
                    .stroke(Stroke::new(1.0, if on { alpha(p.warning, 0.5) } else { p.border }))
                    .min_size(vec2(0.0, 20.0));
                let resp = ui
                    .add(btn)
                    .on_hover_text("Click to toggle the global speed limit · right-click for presets");
                if resp.clicked() {
                    self.settings.speed_limit_enabled = !on;
                }
                resp.context_menu(|ui| {
                    ui.label(RichText::new("Global speed limit").strong());
                    for kib in [256u64, 512, 1024, 2048, 5120, 10240, 25600] {
                        let label = kit::human_speed(kib as f64 * 1024.0);
                        if ui.button(label).clicked() {
                            self.settings.speed_limit_kib = kib;
                            self.settings.speed_limit_enabled = true;
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("No limit").clicked() {
                        self.settings.speed_limit_enabled = false;
                        ui.close();
                    }
                });

                let api = self.api_status();
                let (color, label, tip) = if api.listening {
                    (p.success, "Browser integration", format!("Listening on 127.0.0.1:{}", api.port))
                } else {
                    (p.danger, "Browser integration off", api.error.clone().unwrap_or_default())
                };
                let r = ui
                    .add(egui::Button::new(RichText::new(label).size(12.5).color(p.text_dim)).frame(false))
                    .on_hover_text(tip);
                if r.clicked() {
                    self.settings_tab = super::settings_view::Tab::Browser;
                    self.actions.push(Action::OpenSettings);
                }
                dot(ui, color);
            });
        });
    }
}

fn speed_or_idle(bps: f64) -> String {
    if bps >= 1.0 { kit::human_speed(bps) } else { "Idle".to_owned() }
}

fn toolbar_separator(ui: &mut egui::Ui, p: &Palette) {
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 36.0), egui::Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, p.border));
    ui.add_space(6.0);
}
