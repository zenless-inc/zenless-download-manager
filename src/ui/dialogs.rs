//! Modal dialogs: new download, batch download, delete confirmation and
//! properties.

use super::widgets::{category_badge, category_icon, danger_button, relative_time, status_color, status_icon};
use super::{App, FolderTarget};
use crate::shared::kit;
use crate::shared::theme::{Palette, mix};
use eframe::egui::{self, RichText, Stroke};
use egui_phosphor::regular as icons;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zenless_dm::category::{Category, extension};
use zenless_dm::engine::{BatchRequest, Command, DownloadRequest, NewDownload, ProbeInfo, RequestInfo, StartMode, Status};
use zenless_dm::filename;
use zenless_dm::settings::{MAX_CONNECTIONS, MIN_CONNECTIONS};

const PROBE_DEBOUNCE: Duration = Duration::from_millis(650);

// ---------------------------------------------------------------------------
// New download
// ---------------------------------------------------------------------------

pub enum ProbeState {
    Idle,
    Waiting { token: u64 },
    Done(ProbeInfo),
    Error(String),
}

pub struct NewDownloadDialog {
    pub url: String,
    pub file_name: String,
    pub name_edited: bool,
    pub folder: String,
    pub category: Category,
    pub category_edited: bool,
    pub connections: u32,
    pub request: RequestInfo,
    pub size_hint: Option<u64>,
    pub mime: Option<String>,
    pub source: Option<String>,
    pub page_title: Option<String>,
    pub probe: ProbeState,
    probed_url: String,
    last_edit: Instant,
    focus_url: bool,
}

impl NewDownloadDialog {
    pub fn new(url: &str, folder: PathBuf, connections: u32) -> Self {
        let mut d = Self {
            url: url.trim().to_owned(),
            file_name: String::new(),
            name_edited: false,
            folder: folder.display().to_string(),
            category: Category::Other,
            category_edited: false,
            connections,
            request: RequestInfo::default(),
            size_hint: None,
            mime: None,
            source: None,
            page_title: None,
            probe: ProbeState::Idle,
            probed_url: String::new(),
            // Probe right away when opened with a URL.
            last_edit: Instant::now() - PROBE_DEBOUNCE,
            focus_url: url.trim().is_empty(),
        };
        d.update_name_from_url();
        d
    }

    /// Pre-filled from a browser extension / CLI request (`mode: ask`).
    pub fn from_request(r: &DownloadRequest, folder: PathBuf, connections: u32) -> Self {
        let mut d = Self::new(&r.url, folder, connections);
        d.request = r.request_info();
        d.size_hint = r.size.filter(|&s| s > 0);
        d.mime = r.mime.clone();
        d.source = r.source.clone();
        d.page_title = r.page_title.clone();
        if let Some(f) = r.filename.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            d.file_name = filename::sanitize(f);
            d.name_edited = true;
            d.category = Category::from_file_name(&d.file_name);
        }
        if let Some(dir) = &r.save_dir {
            d.folder = dir.display().to_string();
        }
        d
    }

    fn update_name_from_url(&mut self) {
        if self.name_edited {
            return;
        }
        self.file_name = filename::name_from_url(&self.url)
            .map(|n| filename::ensure_extension(&filename::sanitize(&n), self.mime.as_deref()))
            .unwrap_or_default();
        if !self.category_edited {
            self.category = Category::from_file_name(&self.file_name);
        }
    }

    pub fn on_probe(&mut self, token: u64, result: Result<ProbeInfo, String>) {
        let ProbeState::Waiting { token: t } = self.probe else { return };
        if t != token {
            return;
        }
        match result {
            Ok(info) => {
                if !self.name_edited {
                    self.file_name = info.file_name.clone();
                    if !self.category_edited {
                        self.category = Category::from_file_name(&self.file_name);
                    }
                }
                self.probe = ProbeState::Done(info);
            }
            Err(e) => self.probe = ProbeState::Error(e),
        }
    }

    fn valid_url(&self) -> bool {
        zenless_dm::util::looks_like_url(&self.url)
    }
}

pub enum NewDownloadOutcome {
    Keep,
    Close,
    Add(StartMode),
    PasteMany,
}

impl App {
    pub(super) fn new_download_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut dlg) = self.new_dl.take() else { return };
        // Debounced probe.
        if dlg.valid_url() && dlg.probed_url != dlg.url.trim() {
            if dlg.last_edit.elapsed() >= PROBE_DEBOUNCE {
                dlg.probed_url = dlg.url.trim().to_owned();
                let token = self.engine.probe(&dlg.probed_url, dlg.request.clone());
                dlg.probe = ProbeState::Waiting { token };
            } else {
                ctx.request_repaint_after(PROBE_DEBOUNCE);
            }
        } else if !dlg.valid_url() {
            dlg.probe = ProbeState::Idle;
            dlg.probed_url.clear();
        }
        let duplicate = self.snap.downloads.iter().any(|d| d.url == dlg.url.trim());
        let waiting = self.pending_asks.len() + self.pending_batches.len();

        let mut outcome = NewDownloadOutcome::Keep;
        let modal = egui::Modal::new(egui::Id::new("new_download")).show(ctx, |ui| {
            ui.set_width(580.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::DOWNLOAD_SIMPLE).size(22.0).color(p.accent));
                ui.heading(RichText::new("New download").strong());
                if let Some(src) = dlg.source.as_deref().filter(|s| !s.is_empty()) {
                    let label = match src {
                        "chrome" => "from Chrome".to_owned(),
                        "firefox" => "from Firefox".to_owned(),
                        "cli" => "from command line".to_owned(),
                        other => format!("from {other}"),
                    };
                    kit::pill(ui, &label, p.accent2);
                }
                if waiting > 0 {
                    kit::pill(ui, &format!("{waiting} more waiting"), p.text_dim)
                        .on_hover_text("More downloads from your browser open after this one.");
                }
            });
            if let Some(t) = dlg.page_title.as_deref().filter(|t| !t.is_empty()) {
                ui.add(egui::Label::new(RichText::new(t).size(12.0).color(p.text_dim)).truncate());
            }
            ui.add_space(8.0);

            egui::Grid::new("new_dl_grid").striped(false)
                .num_columns(2)
                .spacing([12.0, 10.0])
                .min_col_width(84.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Address").color(p.text_dim));
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut dlg.url)
                            .hint_text("https://example.com/file.zip")
                            .desired_width(f32::INFINITY),
                    );
                    if dlg.focus_url {
                        r.request_focus();
                        dlg.focus_url = false;
                    }
                    if r.changed() {
                        dlg.last_edit = Instant::now();
                        dlg.update_name_from_url();
                    }
                    ui.end_row();

                    ui.label(RichText::new("File name").color(p.text_dim));
                    ui.horizontal(|ui| {
                        category_badge(ui, p, dlg.category, 24.0);
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut dlg.file_name)
                                .hint_text("Detected automatically")
                                .desired_width(ui.available_width() - 142.0),
                        );
                        if r.changed() {
                            dlg.name_edited = !dlg.file_name.trim().is_empty();
                            if !dlg.category_edited {
                                dlg.category = Category::from_file_name(&dlg.file_name);
                            }
                        }
                        let mut cat = dlg.category;
                        egui::ComboBox::from_id_salt("new_dl_cat")
                            .width(128.0)
                            .selected_text(format!("{}  {}", category_icon(cat), cat.label()))
                            .show_ui(ui, |ui| {
                                for c in Category::ALL {
                                    ui.selectable_value(&mut cat, c, format!("{}  {}", category_icon(c), c.label()));
                                }
                            });
                        if cat != dlg.category {
                            dlg.category = cat;
                            dlg.category_edited = true;
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new("Save to").color(p.text_dim));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut dlg.folder).desired_width(ui.available_width() - 96.0));
                        if ui.button(format!("{}  Browse", icons::FOLDER_OPEN)).clicked() {
                            self.pick_folder(FolderTarget::NewDownload, PathBuf::from(&dlg.folder));
                        }
                    });
                    ui.end_row();

                    ui.label(RichText::new("Connections").color(p.text_dim));
                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(&mut dlg.connections, MIN_CONNECTIONS..=MAX_CONNECTIONS));
                        ui.label(RichText::new("parallel connections").size(12.0).color(p.text_dim));
                    });
                    ui.end_row();
                });
            if self.settings.category_subfolders {
                ui.label(
                    RichText::new(format!(
                        "{}  Saved in the \"{}\" subfolder",
                        icons::FOLDER_SIMPLE,
                        dlg.category.folder_name()
                    ))
                    .size(12.0)
                    .color(p.text_dim),
                );
            }

            ui.add_space(10.0);
            // Probe result card.
            egui::Frame::new()
                .fill(mix(p.surface, p.bg, 0.5))
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| match &dlg.probe {
                        ProbeState::Idle => {
                            let msg = if dlg.url.trim().is_empty() {
                                "Paste or type a link to check it."
                            } else if !dlg.valid_url() {
                                "Enter a valid http:// or https:// address."
                            } else {
                                "Checking shortly…"
                            };
                            ui.label(RichText::new(format!("{}  {msg}", icons::LINK)).color(p.text_dim));
                        }
                        ProbeState::Waiting { .. } => {
                            ui.add(egui::Spinner::new().size(14.0).color(p.accent));
                            ui.label(RichText::new("Checking the link…").color(p.text_dim));
                        }
                        ProbeState::Done(info) => {
                            let size = info
                                .total_size
                                .or(dlg.size_hint)
                                .map_or_else(|| "Unknown size".to_owned(), kit::human_bytes);
                            ui.label(RichText::new(size).strong().color(p.text));
                            ui.label(RichText::new("·").color(p.text_dim));
                            if info.resumable {
                                ui.label(RichText::new(format!("{}  Resumable, multi-connection", icons::CHECK_CIRCLE)).color(p.success));
                            } else {
                                ui.label(
                                    RichText::new(format!("{}  No resume support (single connection)", icons::WARNING_CIRCLE))
                                        .color(p.warning),
                                );
                            }
                            if let Some(m) = &info.mime {
                                ui.label(RichText::new(format!("· {m}")).size(12.0).color(p.text_dim));
                            }
                        }
                        ProbeState::Error(e) => {
                            ui.label(RichText::new(format!("{}  {e}", icons::WARNING_CIRCLE)).color(p.danger));
                        }
                    });
                    if duplicate {
                        ui.label(
                            RichText::new(format!("{}  This link is already in your download list.", icons::INFO))
                                .size(12.0)
                                .color(p.warning),
                        );
                    }
                });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new(format!("{}  Paste many links…", icons::LIST_CHECKS)).color(p.text_dim)).frame(false))
                    .clicked()
                {
                    outcome = NewDownloadOutcome::PasteMany;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = dlg.valid_url() && !dlg.folder.trim().is_empty();
                    let start = ui.add_enabled(
                        ok,
                        egui::Button::new(RichText::new(format!("{}  Start download", icons::DOWNLOAD_SIMPLE)).color(p.accent_fg).strong())
                            .fill(p.accent)
                            .stroke(Stroke::NONE),
                    );
                    if start.clicked() {
                        outcome = NewDownloadOutcome::Add(StartMode::Now);
                    }
                    if ui.add_enabled(ok, egui::Button::new(format!("{}  Add to queue", icons::QUEUE))).clicked() {
                        outcome = NewDownloadOutcome::Add(StartMode::Queue);
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = NewDownloadOutcome::Close;
                    }
                });
            });
            // Enter = start.
            if dlg.valid_url() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                outcome = NewDownloadOutcome::Add(StartMode::Now);
            }
        });
        if modal.should_close() && matches!(outcome, NewDownloadOutcome::Keep) {
            outcome = NewDownloadOutcome::Close;
        }

        match outcome {
            NewDownloadOutcome::Keep => self.new_dl = Some(dlg),
            NewDownloadOutcome::Close => self.after_dialog_closed(),
            NewDownloadOutcome::PasteMany => {
                let mut b = BatchDialog::paste(PathBuf::from(&dlg.folder));
                if !dlg.url.trim().is_empty() {
                    b.paste_text = format!("{}\n", dlg.url.trim());
                    b.parse_paste();
                }
                self.batch = Some(b);
            }
            NewDownloadOutcome::Add(mode) => {
                let probed = match &dlg.probe {
                    ProbeState::Done(i) => Some(i.clone()),
                    _ => None,
                };
                let name = dlg.file_name.trim();
                let lock_name = !name.is_empty() && (dlg.name_edited || probed.is_some());
                let id = self.engine.add(NewDownload {
                    url: dlg.url.trim().to_owned(),
                    file_name: lock_name.then(|| name.to_owned()),
                    save_dir: Some(PathBuf::from(dlg.folder.trim())),
                    category: dlg.category_edited.then_some(dlg.category),
                    connections: Some(dlg.connections),
                    request: dlg.request.clone(),
                    start: mode,
                    size_hint: probed.as_ref().and_then(|p| p.total_size).or(dlg.size_hint),
                    mime: probed.and_then(|p| p.mime).or(dlg.mime.clone()),
                    source: dlg.source.clone(),
                    page_title: dlg.page_title.clone(),
                });
                self.select_only(id);
                self.after_dialog_closed();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

pub struct BatchRow {
    pub url: String,
    pub name: String,
    pub ext: String,
    pub checked: bool,
}

pub struct BatchDialog {
    pub rows: Vec<BatchRow>,
    pub paste_mode: bool,
    pub paste_text: String,
    pub folder: String,
    /// Extensions to show; empty = all.
    pub ext_filter: Vec<String>,
    pub request: RequestInfo,
    pub source: Option<String>,
    pub page_title: Option<String>,
}

impl BatchDialog {
    pub fn paste(folder: PathBuf) -> Self {
        Self {
            rows: Vec::new(),
            paste_mode: true,
            paste_text: String::new(),
            folder: folder.display().to_string(),
            ext_filter: Vec::new(),
            request: RequestInfo::default(),
            source: None,
            page_title: None,
        }
    }

    pub fn from_request(r: &BatchRequest, folder: PathBuf) -> Self {
        let mut b = Self::paste(folder);
        b.paste_mode = false;
        b.request = r.request_info();
        b.source = r.source.clone();
        b.page_title = r.page_title.clone();
        for item in &r.items {
            b.push(&item.url, item.filename.as_deref());
        }
        b
    }

    pub fn from_urls(urls: &[String], folder: PathBuf) -> Self {
        let mut b = Self::paste(folder);
        b.paste_mode = false;
        for u in urls {
            b.push(u, None);
        }
        b
    }

    fn push(&mut self, url: &str, name: Option<&str>) {
        if self.rows.iter().any(|r| r.url == url) {
            return;
        }
        let name = name
            .map(filename::sanitize)
            .or_else(|| filename::name_from_url(url).map(|n| filename::sanitize(&n)))
            .unwrap_or_else(|| filename::DEFAULT_NAME.to_owned());
        let ext = extension(&name).unwrap_or_default();
        self.rows.push(BatchRow { url: url.to_owned(), name, ext, checked: true });
    }

    pub fn parse_paste(&mut self) {
        let urls = zenless_dm::util::extract_urls(&self.paste_text);
        self.rows.retain(|r| urls.contains(&r.url));
        for u in urls {
            self.push(&u, None);
        }
    }

    fn visible(&self, r: &BatchRow) -> bool {
        self.ext_filter.is_empty() || self.ext_filter.contains(&r.ext)
    }
}

impl App {
    pub(super) fn batch_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut b) = self.batch.take() else { return };
        let mut close = false;
        let mut add: Option<bool> = None; // Some(start_queue)
        let modal = egui::Modal::new(egui::Id::new("batch_dialog")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::LIST_CHECKS).size(22.0).color(p.accent));
                ui.heading(RichText::new("Add multiple downloads").strong());
            });
            let sub = match b.page_title.as_deref().filter(|t| !t.is_empty()) {
                Some(t) => format!("{} links from “{t}”", b.rows.len()),
                None => format!("{} links", b.rows.len()),
            };
            ui.label(RichText::new(sub).size(12.5).color(p.text_dim));
            ui.add_space(6.0);

            if b.paste_mode {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut b.paste_text)
                        .hint_text("Paste links here, one per line (any text works — links are picked out automatically)")
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    b.parse_paste();
                }
                ui.add_space(6.0);
            }

            // Extension filter chips.
            let mut exts: BTreeMap<String, usize> = BTreeMap::new();
            for r in &b.rows {
                *exts.entry(r.ext.clone()).or_default() += 1;
            }
            if exts.len() > 1 {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{}  Types", icons::FUNNEL)).size(12.0).color(p.text_dim));
                    let all_sel = b.ext_filter.is_empty();
                    if ui.add(egui::Button::selectable(all_sel, "All")).clicked() {
                        b.ext_filter.clear();
                    }
                    for (ext, n) in &exts {
                        let label = if ext.is_empty() { format!("no ext · {n}") } else { format!(".{ext} · {n}") };
                        let sel = b.ext_filter.contains(ext);
                        if ui.add(egui::Button::selectable(sel, label)).clicked() {
                            if sel {
                                b.ext_filter.retain(|e| e != ext);
                            } else {
                                b.ext_filter.push(ext.clone());
                            }
                        }
                    }
                });
                ui.add_space(4.0);
            }

            ui.horizontal(|ui| {
                if ui.small_button("Select all").clicked() {
                    let filter = b.ext_filter.clone();
                    for r in &mut b.rows {
                        if filter.is_empty() || filter.contains(&r.ext) {
                            r.checked = true;
                        }
                    }
                }
                if ui.small_button("Select none").clicked() {
                    for r in &mut b.rows {
                        r.checked = false;
                    }
                }
                let n = b.rows.iter().filter(|r| r.checked && b.visible(r)).count();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{n} selected")).size(12.0).color(p.text_dim));
                });
            });

            egui::Frame::new()
                .fill(p.input)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, false]).show(ui, |ui| {
                        if b.rows.is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("No links yet").color(p.text_dim));
                            });
                        }
                        let filter = b.ext_filter.clone();
                        for r in b.rows.iter_mut().filter(|r| filter.is_empty() || filter.contains(&r.ext)) {
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut r.checked, "");
                                category_badge(ui, p, Category::from_file_name(&r.name), 20.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.add(egui::Label::new(RichText::new(&r.name).size(13.0)).truncate());
                                    ui.add(egui::Label::new(RichText::new(&r.url).size(11.0).color(p.text_dim)).truncate());
                                });
                            });
                        }
                    });
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Save to").color(p.text_dim));
                ui.add(egui::TextEdit::singleline(&mut b.folder).desired_width(ui.available_width() - 96.0));
                if ui.button(format!("{}  Browse", icons::FOLDER_OPEN)).clicked() {
                    self.pick_folder(FolderTarget::Batch, PathBuf::from(&b.folder));
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = b.rows.iter().filter(|r| r.checked && b.visible(r)).count();
                    let ok = n > 0 && !b.folder.trim().is_empty();
                    let start = ui.add_enabled(
                        ok,
                        egui::Button::new(RichText::new(format!("{}  Download {n}", icons::DOWNLOAD_SIMPLE)).color(p.accent_fg).strong())
                            .fill(p.accent)
                            .stroke(Stroke::NONE),
                    );
                    if start.clicked() {
                        add = Some(true);
                    }
                    if ui.add_enabled(ok, egui::Button::new(format!("{}  Add to queue", icons::QUEUE))).clicked() {
                        add = Some(false);
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        });
        if modal.should_close() && add.is_none() {
            close = true;
        }
        if let Some(start_queue) = add {
            let folder = PathBuf::from(b.folder.trim());
            let mut last = None;
            for r in b.rows.iter().filter(|r| r.checked && b.visible(r)) {
                let mut n = NewDownload::new(r.url.clone(), StartMode::Queue);
                n.save_dir = Some(folder.clone());
                n.request = b.request.clone();
                n.source = b.source.clone();
                n.page_title = b.page_title.clone();
                last = Some(self.engine.add(n));
            }
            if start_queue {
                self.engine.send(Command::StartQueue);
            }
            if let Some(id) = last {
                self.select_only(id);
            }
            self.after_dialog_closed();
        } else if close {
            self.after_dialog_closed();
        } else {
            self.batch = Some(b);
        }
    }

    // -----------------------------------------------------------------------
    // Delete confirmation
    // -----------------------------------------------------------------------

    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(ids) = self.confirm_delete.clone() else { return };
        let snap = self.snap.clone();
        let items: Vec<_> = ids.iter().filter_map(|i| snap.get(*i)).collect();
        if items.is_empty() {
            self.confirm_delete = None;
            return;
        }
        let any_completed = items.iter().any(|d| d.status == Status::Completed);
        let mut result: Option<bool> = None;
        let modal = egui::Modal::new(egui::Id::new("delete_dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::TRASH).size(22.0).color(p.danger));
                let title = if items.len() == 1 { "Remove this download?".to_owned() } else { format!("Remove {} downloads?", items.len()) };
                ui.heading(RichText::new(title).strong());
            });
            ui.add_space(6.0);
            for d in items.iter().take(5) {
                ui.add(egui::Label::new(RichText::new(format!("{}  {}", status_icon(d.status), d.file_name)).color(status_color(p, d.status))).truncate());
            }
            if items.len() > 5 {
                ui.label(RichText::new(format!("…and {} more", items.len() - 5)).color(p.text_dim));
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Unfinished parts (.zdm files) are always removed.").size(12.0).color(p.text_dim));
            if any_completed {
                ui.checkbox(&mut self.delete_files, "Also delete the downloaded files from disk");
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if self.delete_files && any_completed { "Delete files" } else { "Remove" };
                    if danger_button(ui, p, format!("{}  {label}", icons::TRASH)).clicked() {
                        result = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        result = Some(false);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                result = Some(true);
            }
        });
        if modal.should_close() && result.is_none() {
            result = Some(false);
        }
        match result {
            Some(true) => {
                self.engine.send(Command::Remove { ids: ids.clone(), delete_files: self.delete_files && any_completed });
                self.selection.retain(|i| !ids.contains(i));
                self.confirm_delete = None;
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    // -----------------------------------------------------------------------
    // Properties
    // -----------------------------------------------------------------------

    pub(super) fn properties_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(id) = self.properties else { return };
        let snap = self.snap.clone();
        let Some(d) = snap.get(id) else {
            self.properties = None;
            return;
        };
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("properties_dialog")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                category_badge(ui, p, d.category, 30.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add(egui::Label::new(RichText::new(&d.file_name).size(16.0).strong()).truncate());
                    ui.label(RichText::new(format!("{} {}", status_icon(d.status), d.status.label())).color(status_color(p, d.status)));
                });
            });
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 230.0).clamp(200.0, 560.0)).show(ui, |ui| {
                egui::Grid::new("props_grid").striped(false).num_columns(2).spacing([14.0, 7.0]).min_col_width(100.0).show(ui, |ui| {
                    let key = |ui: &mut egui::Ui, k: &str| {
                        ui.label(RichText::new(k).color(p.text_dim));
                    };
                    let mut url = d.url.clone();
                    key(ui, "Address");
                    ui.add(egui::TextEdit::singleline(&mut url).desired_width(400.0));
                    ui.end_row();
                    if let Some(f) = &d.final_url {
                        let mut f = f.clone();
                        key(ui, "Redirected to");
                        ui.add(egui::TextEdit::singleline(&mut f).desired_width(400.0));
                        ui.end_row();
                    }
                    if let Some(r) = &d.request.referrer {
                        key(ui, "Referrer");
                        ui.add(egui::Label::new(r).truncate());
                        ui.end_row();
                    }
                    key(ui, "Saved to");
                    ui.add(egui::Label::new(d.path().display().to_string()).truncate());
                    ui.end_row();
                    key(ui, "Size");
                    ui.label(match d.total_size {
                        Some(t) => format!("{} ({} bytes)", kit::human_bytes(t), t),
                        None => "Unknown".into(),
                    });
                    ui.end_row();
                    key(ui, "Downloaded");
                    ui.label(format!(
                        "{}{}",
                        kit::human_bytes(d.downloaded),
                        d.progress().map_or(String::new(), |f| format!(" ({:.1}%)", f * 100.0))
                    ));
                    ui.end_row();
                    key(ui, "Resumable");
                    ui.label(match d.resumable {
                        Some(true) => "Yes",
                        Some(false) => "No (single connection)",
                        None => "Not checked yet",
                    });
                    ui.end_row();
                    key(ui, "Type");
                    ui.label(format!("{} · {}", d.category.label(), d.mime.as_deref().unwrap_or("unknown MIME type")));
                    ui.end_row();

                    key(ui, "Connections");
                    let mut n = d.connections;
                    let r = ui.add_enabled(
                        !d.status.is_active() && d.status != Status::Completed,
                        egui::Slider::new(&mut n, MIN_CONNECTIONS..=MAX_CONNECTIONS),
                    );
                    if r.changed() {
                        self.engine.send(Command::SetConnections(id, n));
                    }
                    ui.end_row();

                    key(ui, "Speed limit");
                    ui.horizontal(|ui| {
                        let mut on = d.speed_limit.is_some();
                        let mut kib = d.speed_limit.map_or(1024, |l| (l / 1024).max(1));
                        let mut changed = ui.checkbox(&mut on, "").changed();
                        changed |= ui
                            .add_enabled(on, egui::DragValue::new(&mut kib).range(16..=1_048_576).suffix(" KB/s").speed(16.0))
                            .changed();
                        if changed {
                            self.engine.send(Command::SetItemLimit(id, on.then_some(kib * 1024)));
                        }
                    });
                    ui.end_row();

                    key(ui, "Added");
                    ui.label(format!("{} ({})", kit::human_date(d.added_at), relative_time(d.added_at)));
                    ui.end_row();
                    if let Some(c) = d.completed_at {
                        key(ui, "Completed");
                        ui.label(format!("{} ({})", kit::human_date(c), relative_time(c)));
                        ui.end_row();
                    }
                    if let Some(s) = d.source.as_deref() {
                        key(ui, "Source");
                        ui.label(s);
                        ui.end_row();
                    }
                    if let Some(t) = d.page_title.as_deref() {
                        key(ui, "Page");
                        ui.add(egui::Label::new(t).truncate());
                        ui.end_row();
                    }
                    if let Some(e) = &d.error {
                        key(ui, "Last error");
                        ui.add(egui::Label::new(RichText::new(e).color(p.danger)).wrap());
                        ui.end_row();
                    }
                    key(ui, "SHA-256");
                    ui.horizontal(|ui| match (&d.sha256, d.rt.hashing) {
                        (_, Some(f)) => {
                            ui.add(egui::Spinner::new().size(14.0));
                            kit::progress_bar(ui, p, f, p.accent, 160.0, 6.0);
                            ui.label(format!("{:.0}%", f * 100.0));
                        }
                        (Some(h), None) => {
                            ui.add(egui::Label::new(RichText::new(h).monospace().size(11.5)).truncate());
                            if ui.small_button(icons::COPY).on_hover_text("Copy").clicked() {
                                ui.ctx().copy_text(h.clone());
                            }
                        }
                        (None, None) => {
                            if ui
                                .add_enabled(d.status == Status::Completed, egui::Button::new(format!("{}  Compute", icons::HASH)))
                                .on_disabled_hover_text("Available once the download has finished")
                                .clicked()
                            {
                                self.engine.send(Command::Hash(id));
                            }
                        }
                    });
                    ui.end_row();
                });
                if let Some(note) = &d.note {
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!("{}  {note}", icons::INFO)).size(12.0).color(p.warning));
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Copy URL", icons::LINK)).clicked() {
                    ui.ctx().copy_text(d.url.clone());
                }
                if ui.button(format!("{}  Open folder", icons::FOLDER_OPEN)).clicked() {
                    self.reveal(&d.path());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::primary_button(ui, p, "Close").clicked() {
                        close = true;
                    }
                });
            });
        });
        if close || modal.should_close() {
            self.properties = None;
        }
    }
}
