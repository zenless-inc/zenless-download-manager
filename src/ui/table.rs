//! The main download list.

use super::widgets::{category_badge, progress_cell, relative_time, status_color, status_icon};
use super::{Action, App, SortCol};
use crate::shared::kit;
use crate::shared::theme::{Palette, mix};
use eframe::egui::{self, Align, FontId, Layout, RichText, Sense, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;
use zenless_dm::engine::{Download, Id, Status};

const ROW_H: f32 = 40.0;

impl App {
    pub(super) fn table(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let snap = self.snap.clone();
        if snap.downloads.is_empty() {
            self.empty_state(ui, p, true);
            return;
        }
        let rows = self.visible_rows();
        if rows.is_empty() {
            self.empty_state(ui, p, false);
            return;
        }
        let visible_ids: Vec<Id> = rows.iter().map(|&i| snap.downloads[i].id).collect();
        let mut clicked: Option<(Id, egui::Modifiers)> = None;
        let mut double: Option<Id> = None;
        let mut context_for: Option<Id> = None;
        let modifiers = ui.input(|i| i.modifiers);

        ui.scope(|ui| {
            // Theme-derived table colors.
            let v = ui.visuals_mut();
            v.selection.bg_fill = mix(p.bg, p.accent, 0.13);
            v.selection.stroke.color = p.text;
            v.widgets.hovered.bg_fill = mix(p.bg, p.text, 0.045);
            v.faint_bg_color = p.stripe;
            ui.spacing_mut().item_spacing = vec2(10.0, 0.0);

            let table = TableBuilder::new(ui)
                .id_salt("downloads")
                .striped(true)
                .sense(Sense::click())
                .resizable(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(40.0))
                .column(Column::remainder().at_least(180.0).clip(true))
                .column(Column::initial(84.0).at_least(60.0))
                .column(Column::initial(150.0).at_least(90.0))
                .column(Column::initial(92.0).at_least(60.0))
                .column(Column::initial(72.0).at_least(50.0))
                .column(Column::initial(118.0).at_least(80.0))
                .column(Column::initial(96.0).at_least(70.0))
                .auto_shrink([false, false])
                .min_scrolled_height(0.0);

            let header_cols: [(Option<SortCol>, &str); 8] = [
                (None, ""),
                (Some(SortCol::Name), "Name"),
                (Some(SortCol::Size), "Size"),
                (Some(SortCol::Progress), "Progress"),
                (Some(SortCol::Speed), "Speed"),
                (Some(SortCol::Eta), "ETA"),
                (Some(SortCol::Status), "Status"),
                (Some(SortCol::Added), "Added"),
            ];
            let sort = self.sort;
            let mut new_sort = sort;
            table
                .header(30.0, |mut header| {
                    for (col, label) in header_cols {
                        header.col(|ui| {
                            let r = ui.max_rect();
                            ui.painter().hline(
                                (r.left() - 5.0)..=(r.right() + 5.0),
                                r.bottom() - 0.5,
                                egui::Stroke::new(1.0, p.border),
                            );
                            let Some(col) = col else { return };
                            let active = sort.map(|(c, _)| c) == Some(col);
                            let arrow = match sort {
                                Some((c, true)) if c == col => icons::CARET_UP,
                                Some((c, false)) if c == col => icons::CARET_DOWN,
                                _ => "",
                            };
                            let text = RichText::new(format!("{label} {arrow}"))
                                .size(12.0)
                                .strong()
                                .color(if active { p.text } else { p.text_dim });
                            let r = ui.add(egui::Label::new(text).sense(Sense::click()).selectable(false));
                            if r.clicked() {
                                new_sort = match sort {
                                    Some((c, true)) if c == col => Some((col, false)),
                                    Some((c, false)) if c == col => None,
                                    _ => Some((col, true)),
                                };
                            }
                            r.on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("Sort (click again to reverse, third click restores queue order)");
                        });
                    }
                })
                .body(|body| {
                    body.rows(ROW_H, rows.len(), |mut row| {
                        let d = &snap.downloads[rows[row.index()]];
                        let selected = self.selection.contains(&d.id);
                        row.set_selected(selected);
                        row.col(|ui| {
                            if selected {
                                let r = ui.max_rect();
                                let bar = egui::Rect::from_min_size(r.left_top() + vec2(0.0, 7.0), vec2(3.0, r.height() - 14.0));
                                ui.painter().rect_filled(bar, egui::CornerRadius::same(2), p.accent);
                            }
                            ui.add_space(8.0);
                            category_badge(ui, p, d.category, 26.0);
                        });
                        row.col(|ui| name_cell(ui, p, d));
                        row.col(|ui| {
                            let text = d.total_size.map_or_else(|| "—".to_owned(), kit::human_bytes);
                            ui.label(RichText::new(text).size(13.0).color(p.text));
                        });
                        row.col(|ui| progress_cell(ui, p, d));
                        row.col(|ui| {
                            let (text, color) = if d.status.is_active() {
                                (kit::human_speed(d.rt.speed), p.text)
                            } else {
                                ("—".to_owned(), p.text_dim)
                            };
                            ui.label(RichText::new(text).size(13.0).color(color));
                        });
                        row.col(|ui| {
                            let text = if d.status.is_active() { d.rt.eta.map_or("—".into(), kit::human_eta) } else { "—".into() };
                            ui.label(RichText::new(text).size(13.0).color(p.text_dim));
                        });
                        row.col(|ui| {
                            let color = status_color(p, d.status);
                            kit::pill(ui, &format!("{} {}", status_icon(d.status), d.status.label()), color);
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(relative_time(d.added_at)).size(12.5).color(p.text_dim))
                                .on_hover_text(kit::human_date(d.added_at) + " UTC");
                        });
                        let resp = row.response();
                        if resp.clicked() {
                            clicked = Some((d.id, modifiers));
                        }
                        if resp.double_clicked() {
                            double = Some(d.id);
                        }
                        if resp.secondary_clicked() {
                            context_for = Some(d.id);
                        }
                        let id = d.id;
                        resp.context_menu(|ui| self.context_menu(ui, p, id));
                    });
                });
            self.sort = new_sort;
        });

        if let Some(id) = context_for
            && !self.selection.contains(&id)
        {
            self.select_only(id);
        }
        if let Some((id, m)) = clicked {
            self.click_row(id, m, &visible_ids);
        }
        if let Some(id) = double {
            match snap.get(id) {
                Some(d) if d.status == Status::Completed => self.actions.push(Action::Open(id)),
                Some(_) => self.actions.push(Action::Properties(id)),
                None => {}
            }
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, p: &Palette, id: Id) {
        ui.set_min_width(190.0);
        // Right-clicking an unselected row acts on that row only.
        let ids = if self.selection.contains(&id) { self.selected_ids() } else { vec![id] };
        let snap = self.snap.clone();
        let items: Vec<&Download> = ids.iter().filter_map(|i| snap.get(*i)).collect();
        let one = items.len() == 1;
        let completed = items.first().is_some_and(|d| d.status == Status::Completed);
        let can_resume = items.iter().any(|d| d.status.can_resume());
        let can_pause = items.iter().any(|d| d.status.is_active() || d.status == Status::Queued);
        let mut act = |ui: &mut egui::Ui, enabled: bool, icon: &str, label: &str, a: Action| {
            if ui
                .add_enabled(enabled, egui::Button::new(format!("{icon}   {label}")).frame(false))
                .clicked()
            {
                self.actions.push(a);
                ui.close();
            }
        };
        act(ui, one && completed, icons::ARROW_SQUARE_OUT, "Open", Action::Open(id));
        act(ui, one, icons::FOLDER_OPEN, "Open folder", Action::OpenFolder(id));
        ui.separator();
        act(ui, can_resume, icons::PLAY, "Resume", Action::Resume(ids.clone()));
        act(ui, can_pause, icons::PAUSE, "Pause", Action::Pause(ids.clone()));
        act(ui, true, icons::ARROW_CLOCKWISE, "Restart", Action::Restart(ids.clone()));
        ui.separator();
        act(ui, true, icons::LINK, "Copy URL", Action::CopyUrl(ids.clone()));
        act(ui, true, icons::ARROW_UP, "Move up", Action::MoveUp(ids.clone()));
        act(ui, true, icons::ARROW_DOWN, "Move down", Action::MoveDown(ids.clone()));
        act(ui, true, icons::ARROW_LINE_UP, "Move to top", Action::MoveTop(ids.clone()));
        ui.separator();
        act(ui, true, icons::TRASH, "Delete…", Action::AskDelete(ids.clone()));
        act(ui, one, icons::INFO, "Properties", Action::Properties(id));
        if self.sort.is_some() {
            ui.separator();
            ui.label(RichText::new("Queue moves apply to queue order").size(11.0).color(p.text_dim));
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, p: &Palette, no_downloads: bool) {
        ui.vertical_centered(|ui| {
            let h = ui.available_height();
            ui.add_space((h * 0.28).max(20.0));
            let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 42.0, mix(p.bg, p.accent, 0.10));
            ui.painter().circle_filled(rect.center(), 30.0, mix(p.bg, p.accent, 0.18));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if no_downloads { icons::DOWNLOAD_SIMPLE } else { icons::FUNNEL },
                FontId::proportional(34.0),
                p.accent,
            );
            ui.add_space(14.0);
            if no_downloads {
                ui.label(RichText::new("No downloads yet").size(20.0).strong().color(p.text));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Add a link with Ctrl+N, paste one with Ctrl+V, drop a link here,\nor send it from your browser with the Zenless extension.")
                        .color(p.text_dim),
                );
                ui.add_space(14.0);
                if kit::primary_button(ui, p, format!("{}  Add URL", icons::PLUS)).clicked() {
                    self.actions.push(Action::NewDownload(None));
                }
            } else {
                ui.label(RichText::new("Nothing matches").size(20.0).strong().color(p.text));
                ui.add_space(4.0);
                ui.label(RichText::new("No downloads match the current filter or search.").color(p.text_dim));
                ui.add_space(14.0);
                if ui.button("Clear filters").clicked() {
                    self.filter = super::Filter::All;
                    self.search.clear();
                }
            }
        });
    }
}

fn host_of(url: &str) -> String {
    reqwest_host(url).unwrap_or_else(|| url.chars().take(60).collect())
}

fn reqwest_host(url: &str) -> Option<String> {
    let after = url.split_once("://")?.1;
    let host = after.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

fn name_cell(ui: &mut egui::Ui, p: &Palette, d: &Download) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add_space(3.0);
        ui.add(
            egui::Label::new(RichText::new(&d.file_name).size(13.5).color(p.text))
                .truncate()
                .selectable(false),
        );
        let sub = match (&d.error, d.status) {
            (Some(e), Status::Failed) => RichText::new(e).size(11.5).color(p.danger),
            _ => {
                let mut s = host_of(&d.url);
                if d.status.is_active() || d.status == Status::Paused {
                    if let Some(t) = d.total_size {
                        s = format!("{} of {} · {s}", kit::human_bytes(d.downloaded), kit::human_bytes(t));
                    } else if d.downloaded > 0 {
                        s = format!("{} · {s}", kit::human_bytes(d.downloaded));
                    }
                }
                RichText::new(s).size(11.5).color(p.text_dim)
            }
        };
        ui.add(egui::Label::new(sub).truncate().selectable(false));
    });
}
