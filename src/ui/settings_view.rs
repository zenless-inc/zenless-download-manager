//! Settings dialog: General, Network, Browser integration, Appearance, About.

use super::widgets::{dot, relative_time};
use super::{App, FolderTarget};
use crate::shared::kit;
use crate::shared::theme::{Palette, mix};
use eframe::egui::{self, CornerRadius, FontId, RichText, Sense, Stroke, vec2};
use egui_phosphor::regular as icons;
use std::path::PathBuf;
use zenless_dm::settings::{DEFAULT_USER_AGENT, MAX_CONNECTIONS, MIN_CONNECTIONS};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    #[default]
    General,
    Network,
    Browser,
    Appearance,
    About,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::General, Tab::Network, Tab::Browser, Tab::Appearance, Tab::About];
    fn label(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Network => "Network",
            Tab::Browser => "Browser integration",
            Tab::Appearance => "Appearance",
            Tab::About => "About",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Tab::General => icons::SLIDERS_HORIZONTAL,
            Tab::Network => icons::GLOBE,
            Tab::Browser => icons::PUZZLE_PIECE,
            Tab::Appearance => icons::PALETTE,
            Tab::About => icons::INFO,
        }
    }
}

impl App {
    pub(super) fn settings_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.settings_open {
            return;
        }
        let mut close = false;
        let screen = ctx.content_rect();
        let size = vec2((screen.width() - 80.0).clamp(560.0, 860.0), (screen.height() - 80.0).clamp(360.0, 600.0));
        let modal = egui::Modal::new(egui::Id::new("settings_dialog")).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::GEAR_SIX).size(22.0).color(p.accent));
                ui.heading(RichText::new("Settings").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(icons::X).size(16.0)).frame(false))
                        .on_hover_text("Close (Esc)")
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                // Tabs.
                ui.vertical(|ui| {
                    ui.set_width(186.0);
                    for tab in Tab::ALL {
                        if tab_button(ui, p, tab, self.settings_tab == tab).clicked() {
                            self.settings_tab = tab;
                        }
                    }
                });
                ui.add_space(4.0);
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, ui.available_height()), Sense::hover());
                ui.painter().vline(sep.center().x, sep.y_range(), Stroke::new(1.0, p.border));
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("settings_scroll", self.settings_tab as u8))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add_space(2.0);
                            match self.settings_tab {
                                Tab::General => self.settings_general(ui, p),
                                Tab::Network => self.settings_network(ui, p),
                                Tab::Browser => self.settings_browser(ui, p),
                                Tab::Appearance => self.themes.appearance_ui(ui),
                                Tab::About => self.settings_about(ui, p),
                            }
                            ui.add_space(8.0);
                        });
                });
            });
        });
        if close || modal.should_close() {
            self.settings_open = false;
        }
    }

    fn settings_general(&mut self, ui: &mut egui::Ui, p: &Palette) {
        section(ui, p, "Saving files");
        ui.label(RichText::new("Default download folder").color(p.text_dim));
        ui.horizontal(|ui| {
            let mut folder = self.settings.download_dir.display().to_string();
            if ui
                .add(egui::TextEdit::singleline(&mut folder).desired_width(ui.available_width() - 100.0))
                .changed()
            {
                self.settings.download_dir = PathBuf::from(folder);
            }
            if ui.button(format!("{}  Browse", icons::FOLDER_OPEN)).clicked() {
                self.pick_folder(FolderTarget::Settings, self.settings.download_dir.clone());
            }
        });
        ui.checkbox(
            &mut self.settings.category_subfolders,
            "Sort into category subfolders (Video, Music, Documents, …)",
        );
        ui.add_space(10.0);

        section(ui, p, "Downloads");
        egui::Grid::new("general_grid").striped(false).num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Max. simultaneous downloads");
            ui.add(egui::Slider::new(&mut self.settings.max_concurrent, 1..=16));
            ui.end_row();
            ui.label("Connections per download");
            ui.add(egui::Slider::new(&mut self.settings.default_connections, MIN_CONNECTIONS..=MAX_CONNECTIONS));
            ui.end_row();
        });
        ui.checkbox(&mut self.settings.auto_resume, "Resume unfinished downloads when the app starts");
        ui.checkbox(&mut self.settings.confirm_delete, "Ask before removing downloads");
        ui.checkbox(&mut self.settings.notifications, "Show a notification when a download finishes");
        ui.checkbox(
            &mut self.settings.clipboard_monitor,
            "Offer to download links copied to the clipboard",
        );
        ui.add_space(10.0);

        section(ui, p, "System");
        let mut autostart = self.settings.start_with_windows;
        if ui
            .add_enabled(
                cfg!(windows) && !self.demo,
                egui::Checkbox::new(&mut autostart, "Start with Windows (minimized)"),
            )
            .changed()
        {
            match zenless_dm::autostart::set_enabled(autostart) {
                Ok(()) => {
                    self.settings.start_with_windows = autostart;
                    self.autostart_error = None;
                }
                Err(e) => self.autostart_error = Some(e),
            }
        }
        if let Some(e) = &self.autostart_error {
            ui.label(RichText::new(format!("{}  {e}", icons::WARNING_CIRCLE)).color(p.danger));
        }
    }

    fn settings_network(&mut self, ui: &mut egui::Ui, p: &Palette) {
        section(ui, p, "Speed");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.settings.speed_limit_enabled, "Limit total download speed to");
            ui.add_enabled(
                self.settings.speed_limit_enabled,
                egui::DragValue::new(&mut self.settings.speed_limit_kib)
                    .range(16..=10_485_760)
                    .speed(16.0)
                    .suffix(" KB/s"),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Presets").size(12.0).color(p.text_dim));
            for kib in [512u64, 1024, 2048, 5120, 10240, 51200] {
                if ui.small_button(kit::human_speed(kib as f64 * 1024.0)).clicked() {
                    self.settings.speed_limit_kib = kib;
                    self.settings.speed_limit_enabled = true;
                }
            }
        });
        ui.label(
            RichText::new("Individual downloads can have their own limit in Properties.")
                .size(12.0)
                .color(p.text_dim),
        );
        ui.add_space(10.0);

        section(ui, p, "Connections");
        egui::Grid::new("net_grid").striped(false).num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Retries per connection");
            ui.add(egui::Slider::new(&mut self.settings.max_retries, 0..=30));
            ui.end_row();
            ui.label("Timeout");
            ui.add(egui::Slider::new(&mut self.settings.timeout_secs, 5..=120).suffix(" s"));
            ui.end_row();
        });
        ui.label(
            RichText::new("Failed connections retry with exponential backoff (1 s, 2 s, 4 s … 30 s) without losing progress.")
                .size(12.0)
                .color(p.text_dim),
        );
        ui.add_space(10.0);

        section(ui, p, "User agent");
        ui.add(
            egui::TextEdit::multiline(&mut self.settings.user_agent)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui.small_button("Reset to default").clicked() {
                self.settings.user_agent = DEFAULT_USER_AGENT.to_owned();
            }
            ui.label(
                RichText::new("Downloads sent from the browser use the browser's own user agent, referrer and cookies.")
                    .size(12.0)
                    .color(p.text_dim),
            );
        });
    }

    fn settings_browser(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let api = self.api_status();
        section(ui, p, "Status");
        kit::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if api.listening {
                    dot(ui, p.success);
                    ui.label(RichText::new(format!("Listening on 127.0.0.1:{}", api.port)).strong());
                } else {
                    dot(ui, p.danger);
                    ui.label(RichText::new("Browser integration is unavailable").strong().color(p.danger));
                }
            });
            if let Some(e) = &api.error {
                ui.label(RichText::new(e).size(12.5).color(p.text_dim));
            }
            ui.add_space(4.0);
            match &api.last_extension_contact {
                Some(c) => ui.label(format!(
                    "{}  Last contact from an extension: {} · {}",
                    icons::PLUGS_CONNECTED,
                    c.client,
                    relative_time(c.at)
                )),
                None => ui.label(
                    RichText::new(format!("{}  No browser extension has connected yet.", icons::PLUGS))
                        .color(p.text_dim),
                ),
            };
            if let Some(c) = &api.last_contact
                && api.last_extension_contact.as_ref() != Some(c)
            {
                ui.label(
                    RichText::new(format!("Last request: {} · {}", c.client, relative_time(c.at)))
                        .size(12.0)
                        .color(p.text_dim),
                );
            }
        });
        ui.add_space(12.0);

        section(ui, p, "Install the browser extension");
        ui.label(
            "The Zenless Browser Integration extension sends downloads from your browser to this app, \
             together with the page's cookies and referrer so protected links work.",
        );
        ui.add_space(6.0);
        let ext_root = dirs::data_local_dir()
            .unwrap_or_default()
            .join("Programs")
            .join("Zenless")
            .join("Browser Extensions");
        kit::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("{}  Chrome, Edge, Brave, Opera", icons::BROWSER)).strong());
            ui.label("1. Install it with Zenless Setup, or download it from the Zenless website.");
            ui.label("2. Open chrome://extensions (edge://extensions) and turn on Developer mode.");
            ui.label(format!("3. Click “Load unpacked” and choose {}", ext_root.join("Chrome").display()));
        });
        ui.add_space(6.0);
        kit::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("{}  Firefox", icons::BROWSER)).strong());
            ui.label("1. Install it with Zenless Setup, or download it from the Zenless website.");
            ui.label(format!(
                "2. Open {} (drag it into Firefox) and confirm.",
                ext_root.join("zenless-firefox-extension.xpi").display()
            ));
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if kit::primary_button(ui, p, format!("{}  Get the extensions", icons::ARROW_SQUARE_OUT)).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(zenless_dm::DOWNLOAD_PAGE));
            }
            if ext_root.exists() && ui.button(format!("{}  Open extensions folder", icons::FOLDER_OPEN)).clicked() {
                self.open_path(&ext_root);
            }
        });
        ui.add_space(12.0);
        section(ui, p, "Local API");
        ui.label(
            RichText::new(
                "GET  /ping   /status\nPOST /download   /batch   /focus   /quit\n\
                 127.0.0.1 only · extension origins only · POST needs X-Zenless-Client",
            )
            .monospace()
            .size(12.0)
            .color(p.text_dim),
        );
    }

    fn settings_about(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(14), p.accent);
            ui.painter().text(
                rect.center() + vec2(0.0, 1.0),
                egui::Align2::CENTER_CENTER,
                icons::DOWNLOAD_SIMPLE,
                FontId::proportional(32.0),
                p.accent_fg,
            );
            ui.vertical(|ui| {
                ui.label(RichText::new(zenless_dm::APP_NAME).size(20.0).strong());
                ui.label(RichText::new(format!("Version {}", zenless_dm::VERSION)).color(p.text_dim));
                ui.label(RichText::new("MIT License · Copyright (c) 2026 Zenless").size(12.0).color(p.text_dim));
            });
        });
        ui.add_space(10.0);
        ui.label("A fast, multi-connection download manager with IDM-style dynamic segmentation, a queue, speed limits and browser integration.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.hyperlink_to(format!("{}  Website", icons::GLOBE), zenless_dm::WEBSITE);
            ui.hyperlink_to(format!("{}  Downloads & extensions", icons::DOWNLOAD_SIMPLE), zenless_dm::DOWNLOAD_PAGE);
            ui.hyperlink_to(format!("{}  Source code", icons::ARROW_SQUARE_OUT), zenless_dm::REPOSITORY);
        });
        ui.add_space(12.0);
        section(ui, p, "Keyboard shortcuts");
        egui::Grid::new("shortcuts").num_columns(2).spacing([24.0, 5.0]).striped(true).show(ui, |ui| {
            for (k, v) in [
                ("Ctrl+N", "New download"),
                ("Ctrl+V", "Paste a link (outside text fields)"),
                ("Ctrl+A", "Select all visible downloads"),
                ("Space", "Pause / resume selection"),
                ("Delete", "Remove selection"),
                ("Enter", "Open finished file / properties"),
                ("Esc", "Clear selection / close dialog"),
            ] {
                ui.label(RichText::new(k).monospace().color(p.accent));
                ui.label(v);
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        section(ui, p, "Data");
        let dir = zenless_dm::settings::data_dir();
        ui.horizontal(|ui| {
            ui.label(RichText::new(dir.display().to_string()).monospace().size(12.0).color(p.text_dim));
            if ui.small_button("Open").clicked() {
                self.open_path(&dir);
            }
        });
    }
}

fn section(ui: &mut egui::Ui, p: &Palette, title: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(title).size(15.0).strong().color(p.text));
    ui.add_space(2.0);
}

fn tab_button(ui: &mut egui::Ui, p: &Palette, tab: Tab, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(8), mix(p.surface, p.accent, 0.14));
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(8), mix(p.surface, p.text, 0.05));
    }
    painter.text(
        rect.left_center() + vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        tab.icon(),
        FontId::proportional(17.0),
        if selected { p.accent } else { p.text_dim },
    );
    painter.text(
        rect.left_center() + vec2(38.0, 0.0),
        egui::Align2::LEFT_CENTER,
        tab.label(),
        FontId::proportional(14.0),
        if selected { p.text } else { mix(p.text, p.text_dim, 0.3) },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
