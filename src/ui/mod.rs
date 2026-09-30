//! The egui front-end. It never blocks: it sends [`Command`]s to the engine,
//! reads the latest [`Snapshot`] every frame and drains [`UiEvent`]s.

mod details;
mod dialogs;
mod settings_view;
mod sidebar;
mod table;
mod toasts;
mod toolbar;
mod widgets;

use crate::Cli;
use crate::shared::kit;
use crate::shared::theme::{Palette, ThemeManager, alpha};
use dialogs::{BatchDialog, NewDownloadDialog};
use eframe::egui::{self, vec2};
use egui_phosphor::regular as icons;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use toasts::{ToastAction, ToastKind, Toasts};
use zenless_dm::api::{self, ApiStatus, EngineBackend};
use zenless_dm::category::Category;
use zenless_dm::engine::{Command, Download, DownloadRequest, Engine, EngineConfig, Id, Notifier, Snapshot, Status, UiEvent};
use zenless_dm::settings::Settings;

/// Sidebar filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Downloading,
    Queued,
    Paused,
    Completed,
    Failed,
    Category(Category),
}

impl Filter {
    pub fn matches(self, d: &Download) -> bool {
        match self {
            Filter::All => true,
            Filter::Downloading => d.status.is_active(),
            Filter::Queued => d.status == Status::Queued,
            Filter::Paused => d.status == Status::Paused,
            Filter::Completed => d.status == Status::Completed,
            Filter::Failed => d.status == Status::Failed,
            Filter::Category(c) => d.category == c,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortCol {
    Name,
    Size,
    Progress,
    Speed,
    Eta,
    Status,
    Added,
}

/// Deferred UI actions (collected while drawing, applied after).
#[derive(Clone, Debug)]
pub enum Action {
    NewDownload(Option<String>),
    Resume(Vec<Id>),
    Pause(Vec<Id>),
    PauseAll,
    Restart(Vec<Id>),
    AskDelete(Vec<Id>),
    Open(Id),
    OpenFolder(Id),
    CopyUrl(Vec<Id>),
    MoveUp(Vec<Id>),
    MoveDown(Vec<Id>),
    MoveTop(Vec<Id>),
    Properties(Id),
    OpenSettings,
}

/// Where a folder picked with the native dialog goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderTarget {
    NewDownload,
    Batch,
    Settings,
}

pub struct App {
    themes: ThemeManager,
    shot: kit::AutoScreenshot,
    engine: Engine,
    events: Receiver<UiEvent>,
    snap: Arc<Snapshot>,
    api_status: Arc<Mutex<ApiStatus>>,
    clipboard_enabled: Arc<AtomicBool>,
    /// UI copy of the settings; pushed to the engine when it changes.
    settings: Settings,
    sent_settings: Settings,
    demo: bool,
    start_minimized: bool,
    frame_no: u64,

    filter: Filter,
    search: String,
    search_focused: bool,
    sort: Option<(SortCol, bool)>,
    selection: BTreeSet<Id>,
    anchor: Option<Id>,
    focus: Option<Id>,

    new_dl: Option<NewDownloadDialog>,
    pending_asks: VecDeque<DownloadRequest>,
    batch: Option<BatchDialog>,
    confirm_delete: Option<Vec<Id>>,
    delete_files: bool,
    properties: Option<Id>,
    settings_open: bool,
    settings_tab: settings_view::Tab,
    autostart_error: Option<String>,
    folder_pick: Option<(FolderTarget, Receiver<Option<PathBuf>>)>,

    toasts: Toasts,
    actions: Vec<Action>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, config: EngineConfig, cli: Cli, demo: bool) -> Result<Self, String> {
        let themes = ThemeManager::new(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let notify: Notifier = Arc::new(move || ctx.request_repaint());
        let (tx, rx): (Sender<UiEvent>, Receiver<UiEvent>) = std::sync::mpsc::channel();
        let mut settings = config.settings.clone().normalized();
        let engine = Engine::start(config, tx.clone(), notify.clone()).map_err(|e| format!("Could not start the download engine: {e}"))?;

        let api_status = Arc::new(Mutex::new(ApiStatus { port: api::PORT, ..Default::default() }));
        if demo {
            if let Ok(mut s) = api_status.lock() {
                s.error = Some("Demo mode: the local API server is disabled.".into());
            }
        } else {
            let backend = Arc::new(EngineBackend {
                engine: engine.clone(),
                events: tx.clone(),
                notify: notify.clone(),
                status: api_status.clone(),
            });
            let result = api::start(backend, api::PORT);
            if let Ok(mut s) = api_status.lock() {
                match result {
                    Ok(()) => s.listening = true,
                    Err(e) => {
                        s.error = Some(format!(
                            "Port {} is already in use or blocked ({e}). Downloads still work, but browser extensions can't reach the app.",
                            api::PORT
                        ))
                    }
                }
            }
        }

        let clipboard_enabled = Arc::new(AtomicBool::new(settings.clipboard_monitor && !demo));
        zenless_dm::clipboard::spawn_monitor(clipboard_enabled.clone(), tx.clone(), notify);
        if !demo {
            settings.start_with_windows = zenless_dm::autostart::is_enabled();
        }

        let mut app = Self {
            themes,
            shot: kit::AutoScreenshot::from_env(),
            snap: engine.snapshot(),
            engine,
            events: rx,
            api_status,
            clipboard_enabled,
            sent_settings: settings.clone(),
            settings,
            demo,
            start_minimized: cli.minimized,
            frame_no: 0,
            filter: Filter::All,
            search: String::new(),
            search_focused: false,
            sort: None,
            selection: BTreeSet::new(),
            anchor: None,
            focus: None,
            new_dl: None,
            pending_asks: VecDeque::new(),
            batch: None,
            confirm_delete: None,
            delete_files: false,
            properties: None,
            settings_open: false,
            settings_tab: settings_view::Tab::General,
            autostart_error: None,
            folder_pick: None,
            toasts: Toasts::default(),
            actions: Vec::new(),
        };
        match cli.urls.len() {
            0 => {}
            1 => app.pending_asks.push_back(DownloadRequest {
                url: cli.urls[0].clone(),
                source: Some("cli".into()),
                ..Default::default()
            }),
            _ => app.batch = Some(BatchDialog::from_urls(&cli.urls, app.settings.download_dir.clone())),
        }
        if demo {
            app.demo_view();
        }
        Ok(app)
    }

    /// Demo mode: select a busy download and optionally open a dialog given by
    /// `ZENLESS_DEMO_VIEW` (new, batch, properties, delete, settings,
    /// settings-network, settings-browser, settings-appearance, settings-about)
    /// so every screen can be screenshotted.
    fn demo_view(&mut self) {
        if let Some(d) = self.snap.downloads.iter().find(|d| d.status == Status::Downloading) {
            let id = d.id;
            self.select_only(id);
        }
        let view = std::env::var("ZENLESS_DEMO_VIEW").unwrap_or_default();
        let dir = self.settings.download_dir.clone();
        let tab = |t| (true, t);
        let (open_settings, tab) = match view.as_str() {
            "settings" => tab(settings_view::Tab::General),
            "settings-network" => tab(settings_view::Tab::Network),
            "settings-browser" => tab(settings_view::Tab::Browser),
            "settings-appearance" => tab(settings_view::Tab::Appearance),
            "settings-about" => tab(settings_view::Tab::About),
            _ => (false, settings_view::Tab::General),
        };
        self.settings_open = open_settings;
        self.settings_tab = tab;
        match view.as_str() {
            "new" => {
                let req = DownloadRequest {
                    url: "https://cdn.example-media.net/films/Coastline%20Timelapse%208K.mp4".into(),
                    referrer: Some("https://example-media.net/watch/coastline".into()),
                    source: Some("chrome".into()),
                    page_title: Some("Coastline Timelapse 8K — Example Media".into()),
                    ..Default::default()
                };
                self.new_dl = Some(NewDownloadDialog::from_request(&req, dir, self.settings.default_connections));
            }
            "batch" => {
                let items = [
                    "https://files.example.org/album/01%20Sunrise.flac",
                    "https://files.example.org/album/02%20Low%20Tide.flac",
                    "https://files.example.org/album/03%20Driftwood.flac",
                    "https://files.example.org/album/cover.jpg",
                    "https://files.example.org/album/booklet.pdf",
                    "https://files.example.org/album/lyrics.txt",
                ];
                let req = zenless_dm::engine::BatchRequest {
                    items: items
                        .iter()
                        .map(|u| zenless_dm::engine::BatchItem { url: (*u).into(), filename: None })
                        .collect(),
                    page_title: Some("Tidal Lines — Example Music".into()),
                    source: Some("firefox".into()),
                    ..Default::default()
                };
                self.batch = Some(BatchDialog::from_request(&req, dir));
            }
            "properties" => {
                self.properties = self.snap.downloads.iter().find(|d| d.sha256.is_some()).map(|d| d.id);
            }
            "delete" => self.confirm_delete = Some(self.selected_ids()),
            _ => {}
        }
    }

    // -- small helpers -----------------------------------------------------

    fn api_status(&self) -> ApiStatus {
        self.api_status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn selected_ids(&self) -> Vec<Id> {
        // Keep list order.
        self.snap
            .downloads
            .iter()
            .filter(|d| self.selection.contains(&d.id))
            .map(|d| d.id)
            .collect()
    }

    fn selected_downloads(&self) -> Vec<Download> {
        self.snap
            .downloads
            .iter()
            .filter(|d| self.selection.contains(&d.id))
            .cloned()
            .collect()
    }

    fn select_only(&mut self, id: Id) {
        self.selection.clear();
        self.selection.insert(id);
        self.anchor = Some(id);
        self.focus = Some(id);
    }

    fn click_row(&mut self, id: Id, m: egui::Modifiers, visible: &[Id]) {
        if m.shift
            && let Some(a) = self.anchor
            && let (Some(i), Some(j)) = (visible.iter().position(|&x| x == a), visible.iter().position(|&x| x == id))
        {
            if !m.command {
                self.selection.clear();
            }
            let (lo, hi) = if i <= j { (i, j) } else { (j, i) };
            self.selection.extend(visible[lo..=hi].iter().copied());
            self.focus = Some(id);
            return;
        }
        if m.command {
            if !self.selection.remove(&id) {
                self.selection.insert(id);
            }
            self.anchor = Some(id);
            self.focus = Some(id);
            return;
        }
        self.select_only(id);
    }

    /// Indices into `snap.downloads`, filtered and sorted.
    fn visible_rows(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        let mut rows: Vec<usize> = self
            .snap
            .downloads
            .iter()
            .enumerate()
            .filter(|(_, d)| self.filter.matches(d))
            .filter(|(_, d)| q.is_empty() || d.file_name.to_lowercase().contains(&q) || d.url.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        if let Some((col, asc)) = self.sort {
            let dl = &self.snap.downloads;
            rows.sort_by(|&a, &b| {
                let (x, y) = (&dl[a], &dl[b]);
                let ord = match col {
                    SortCol::Name => x.file_name.to_lowercase().cmp(&y.file_name.to_lowercase()),
                    SortCol::Size => x.total_size.cmp(&y.total_size),
                    SortCol::Progress => x.progress().unwrap_or(0.0).total_cmp(&y.progress().unwrap_or(0.0)),
                    SortCol::Speed => x.rt.speed.total_cmp(&y.rt.speed),
                    SortCol::Eta => x.rt.eta.unwrap_or(f64::MAX).total_cmp(&y.rt.eta.unwrap_or(f64::MAX)),
                    SortCol::Status => status_rank(x.status).cmp(&status_rank(y.status)),
                    SortCol::Added => x.added_at.cmp(&y.added_at).then(x.id.cmp(&y.id)),
                };
                if asc { ord } else { ord.reverse() }
            });
        }
        rows
    }

    /// The download shown in the details panel.
    fn focused_download(&self) -> Option<Download> {
        let id = self
            .focus
            .filter(|f| self.selection.contains(f))
            .or_else(|| self.selection.iter().next().copied())?;
        self.snap.get(id).cloned()
    }

    fn prune_selection(&mut self) {
        let visible: BTreeSet<Id> = self.visible_rows().into_iter().map(|i| self.snap.downloads[i].id).collect();
        self.selection.retain(|id| visible.contains(id));
        if self.focus.is_some_and(|f| !visible.contains(&f)) {
            self.focus = None;
        }
    }

    fn any_dialog_open(&self) -> bool {
        self.new_dl.is_some()
            || self.batch.is_some()
            || self.confirm_delete.is_some()
            || self.properties.is_some()
            || self.settings_open
    }

    fn open_path(&mut self, path: &Path) {
        if let Err(e) = zenless_dm::util::open_path(path) {
            self.toasts.push(ToastKind::Error, "Could not open", format!("{}: {e}", path.display()), vec![]);
        }
    }

    fn reveal(&mut self, path: &Path) {
        if let Err(e) = zenless_dm::util::reveal_in_folder(path) {
            self.toasts.push(ToastKind::Error, "Could not open the folder", e.to_string(), vec![]);
        }
    }

    fn pick_folder(&mut self, target: FolderTarget, start: PathBuf) {
        if self.folder_pick.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        // The native dialog is modal and blocking: run it off the UI thread.
        let spawned = std::thread::Builder::new().name("zdm-folder-dialog".into()).spawn(move || {
            let mut dlg = rfd::FileDialog::new().set_title("Choose a folder");
            if start.is_dir() {
                dlg = dlg.set_directory(&start);
            }
            let _ = tx.send(dlg.pick_folder());
        });
        if spawned.is_ok() {
            self.folder_pick = Some((target, rx));
        }
    }

    fn poll_folder_pick(&mut self) {
        let Some((target, rx)) = &self.folder_pick else { return };
        let target = *target;
        match rx.try_recv() {
            Ok(Some(dir)) => {
                match target {
                    FolderTarget::NewDownload => {
                        if let Some(d) = &mut self.new_dl {
                            d.folder = dir.display().to_string();
                        }
                    }
                    FolderTarget::Batch => {
                        if let Some(b) = &mut self.batch {
                            b.folder = dir.display().to_string();
                        }
                    }
                    FolderTarget::Settings => self.settings.download_dir = dir,
                }
                self.folder_pick = None;
            }
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.folder_pick = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn bring_to_front(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn open_new_download(&mut self, url: Option<String>) {
        let url = url.or_else(clipboard_url).unwrap_or_default();
        self.new_dl = Some(NewDownloadDialog::new(&url, self.settings.download_dir.clone(), self.settings.default_connections));
    }

    /// Opens the next queued "ask" request, if any dialog slot is free.
    fn after_dialog_closed(&mut self) {
        self.new_dl = None;
        self.batch = None;
    }

    fn open_pending_ask(&mut self, ctx: &egui::Context) {
        if self.new_dl.is_some() || self.batch.is_some() {
            return;
        }
        if let Some(req) = self.pending_asks.pop_front() {
            self.new_dl = Some(NewDownloadDialog::from_request(
                &req,
                self.settings.download_dir.clone(),
                self.settings.default_connections,
            ));
            self.bring_to_front(ctx);
        }
    }

    /// URLs pasted or dropped onto the window.
    fn accept_urls(&mut self, urls: Vec<String>) {
        match urls.len() {
            0 => {}
            1 => {
                let url = urls.into_iter().next();
                self.new_dl = Some(NewDownloadDialog::new(
                    url.as_deref().unwrap_or_default(),
                    self.settings.download_dir.clone(),
                    self.settings.default_connections,
                ));
            }
            _ => self.batch = Some(BatchDialog::from_urls(&urls, self.settings.download_dir.clone())),
        }
    }

    // -- events, shortcuts, drag & drop -----------------------------------------

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                UiEvent::Finished { name, path, .. } => {
                    if self.settings.notifications {
                        self.toasts.push(
                            ToastKind::Success,
                            "Download complete",
                            name,
                            vec![
                                ("Open".into(), ToastAction::OpenFile(path.clone())),
                                ("Show in folder".into(), ToastAction::ShowInFolder(path)),
                            ],
                        );
                        if !ctx.input(|i| i.focused) {
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                                egui::UserAttentionType::Informational,
                            ));
                        }
                    }
                }
                UiEvent::Failed { name, error, .. } => {
                    self.toasts.push(ToastKind::Error, format!("Failed: {name}"), error, vec![]);
                }
                UiEvent::Ask(req) => {
                    self.pending_asks.push_back(req);
                    self.bring_to_front(ctx);
                }
                UiEvent::Batch(req) => {
                    self.batch = Some(BatchDialog::from_request(&req, self.settings.download_dir.clone()));
                    self.new_dl = None;
                    self.bring_to_front(ctx);
                }
                UiEvent::Focus => self.bring_to_front(ctx),
                UiEvent::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                UiEvent::Probe { token, result } => {
                    if let Some(d) = &mut self.new_dl {
                        d.on_probe(token, result);
                    }
                }
                UiEvent::Clipboard(url) => {
                    let known = self.snap.downloads.iter().any(|d| d.url == url);
                    if !known && self.new_dl.is_none() {
                        let name = zenless_dm::filename::name_from_url(&url).unwrap_or_else(|| url.clone());
                        self.toasts.push(
                            ToastKind::Clipboard,
                            "Download copied link?",
                            name,
                            vec![("Download…".into(), ToastAction::DownloadUrl(url))],
                        );
                    }
                }
                UiEvent::Hashed { name, result, .. } => match result {
                    Ok(h) => self.toasts.push(
                        ToastKind::Info,
                        format!("SHA-256 · {name}"),
                        h.clone(),
                        vec![("Copy".into(), ToastAction::CopyText(h))],
                    ),
                    Err(e) => self.toasts.push(ToastKind::Error, "Hashing failed", e, vec![]),
                },
                UiEvent::Info(msg) => self.toasts.push(ToastKind::Info, msg, "", vec![]),
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.any_dialog_open() {
            return;
        }
        let text_focused = ctx.memory(|m| m.focused().is_some()) && self.search_focused;
        let mut pasted: Option<String> = None;
        let (ctrl_n, ctrl_a, del, space, enter, esc) = ctx.input_mut(|i| {
            let ctrl_n = i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::N));
            if !text_focused {
                for e in &i.events {
                    if let egui::Event::Paste(t) = e {
                        pasted = Some(t.clone());
                    }
                }
            }
            if text_focused {
                return (ctrl_n, false, false, false, false, false);
            }
            (
                ctrl_n,
                i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::A)),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Space),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if ctrl_n {
            self.actions.push(Action::NewDownload(None));
        }
        if ctrl_a {
            let rows = self.visible_rows();
            self.selection = rows.iter().map(|&i| self.snap.downloads[i].id).collect();
        }
        if del && !self.selection.is_empty() {
            self.actions.push(Action::AskDelete(self.selected_ids()));
        }
        if space && !self.selection.is_empty() {
            let sel = self.selected_downloads();
            if sel.iter().any(|d| d.status.is_active() || d.status == Status::Queued) {
                self.actions.push(Action::Pause(self.selected_ids()));
            } else {
                self.actions.push(Action::Resume(self.selected_ids()));
            }
        }
        if enter && let Some(d) = self.focused_download() {
            self.actions.push(if d.status == Status::Completed { Action::Open(d.id) } else { Action::Properties(d.id) });
        }
        if esc {
            self.selection.clear();
        }
        if let Some(text) = pasted {
            let urls = zenless_dm::util::extract_urls(&text);
            self.accept_urls(urls);
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context, p: &Palette) {
        let (hovering, dropped) = ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
        if hovering {
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop_overlay")));
            painter.rect_filled(rect, 0.0, alpha(p.bg, 0.82));
            painter.rect_stroke(
                rect.shrink(12.0),
                egui::CornerRadius::same(14),
                egui::Stroke::new(2.0, p.accent),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center() - vec2(0.0, 18.0),
                egui::Align2::CENTER_CENTER,
                icons::DOWNLOAD_SIMPLE,
                egui::FontId::proportional(44.0),
                p.accent,
            );
            painter.text(
                rect.center() + vec2(0.0, 26.0),
                egui::Align2::CENTER_CENTER,
                "Drop links, .url shortcuts or text files to download",
                egui::FontId::proportional(16.0),
                p.text,
            );
        }
        if dropped.is_empty() {
            return;
        }
        let mut urls: Vec<String> = Vec::new();
        for f in dropped {
            let mut text = String::new();
            let path = f.path().to_path_buf();
            // Only small link-ish files (.url shortcuts, lists of links) are read.
            let small = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() < 2 * 1024 * 1024);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if small
                && matches!(ext.as_str(), "url" | "txt" | "html" | "htm" | "webloc" | "lst" | "")
                && let Ok(bytes) = f.bytes()
            {
                text.push_str(&String::from_utf8_lossy(&bytes));
            }
            text.push('\n');
            text.push_str(&path.to_string_lossy());
            for u in zenless_dm::util::extract_urls(&text) {
                if !urls.contains(&u) {
                    urls.push(u);
                }
            }
        }
        if urls.is_empty() {
            self.toasts.push(ToastKind::Info, "Nothing to download", "No http(s) links found in what was dropped.", vec![]);
        } else if !self.any_dialog_open() {
            self.accept_urls(urls);
        }
    }

    fn apply_actions(&mut self, ctx: &egui::Context) {
        for a in std::mem::take(&mut self.actions) {
            match a {
                Action::NewDownload(url) => self.open_new_download(url),
                Action::Resume(ids) => {
                    self.engine.send(Command::Resume(ids));
                }
                Action::Pause(ids) => {
                    self.engine.send(Command::Pause(ids));
                }
                Action::PauseAll => {
                    self.engine.send(Command::PauseAll);
                }
                Action::Restart(ids) => {
                    self.engine.send(Command::Restart(ids));
                }
                Action::AskDelete(ids) => {
                    if ids.is_empty() {
                        continue;
                    }
                    if self.settings.confirm_delete {
                        self.delete_files = false;
                        self.confirm_delete = Some(ids);
                    } else {
                        self.selection.retain(|i| !ids.contains(i));
                        self.engine.send(Command::Remove { ids, delete_files: false });
                    }
                }
                Action::Open(id) => {
                    if let Some(d) = self.snap.get(id) {
                        let path = d.path();
                        if path.exists() {
                            self.open_path(&path);
                        } else {
                            self.toasts.push(ToastKind::Error, "File not found", path.display().to_string(), vec![]);
                        }
                    }
                }
                Action::OpenFolder(id) => {
                    if let Some(d) = self.snap.get(id) {
                        let path = if d.status == Status::Completed { d.path() } else { d.dir() };
                        self.reveal(&path);
                    }
                }
                Action::CopyUrl(ids) => {
                    let urls: Vec<String> = ids.iter().filter_map(|i| self.snap.get(*i)).map(|d| d.url.clone()).collect();
                    ctx.copy_text(urls.join("\n"));
                }
                Action::MoveUp(ids) => {
                    self.engine.send(Command::MoveUp(ids));
                }
                Action::MoveDown(ids) => {
                    self.engine.send(Command::MoveDown(ids));
                }
                Action::MoveTop(ids) => {
                    self.engine.send(Command::MoveTop(ids));
                }
                Action::Properties(id) => self.properties = Some(id),
                Action::OpenSettings => self.settings_open = true,
            }
        }
    }

    fn handle_toast_actions(&mut self, actions: Vec<ToastAction>, ctx: &egui::Context) {
        for a in actions {
            match a {
                ToastAction::OpenFile(p) => self.open_path(&p),
                ToastAction::ShowInFolder(p) => self.reveal(&p),
                ToastAction::DownloadUrl(u) => {
                    if !self.any_dialog_open() {
                        self.open_new_download(Some(u));
                        self.bring_to_front(ctx);
                    }
                }
                ToastAction::CopyText(t) => ctx.copy_text(t),
            }
        }
    }

    /// Pushes changed settings to the engine (which saves them, debounced).
    fn sync_settings(&mut self) {
        if self.settings != self.sent_settings {
            self.clipboard_enabled
                .store(self.settings.clipboard_monitor && !self.demo, Ordering::Relaxed);
            self.engine.send(Command::SetSettings(Box::new(self.settings.clone())));
            self.sent_settings = self.settings.clone();
        }
    }
}

fn status_rank(s: Status) -> u8 {
    match s {
        Status::Downloading => 0,
        Status::Connecting => 1,
        Status::Queued => 2,
        Status::Paused => 3,
        Status::Failed => 4,
        Status::Completed => 5,
    }
}

/// A single URL currently on the clipboard (to pre-fill "New download").
fn clipboard_url() -> Option<String> {
    let text = arboard::Clipboard::new().ok()?.get_text().ok()?;
    let t = text.trim();
    zenless_dm::util::looks_like_url(t).then(|| t.to_owned())
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.themes.poll(ctx);
        // Events are drained here, not in `ui()`: eframe only runs `logic()` while
        // the window is minimized or hidden, and API requests like /focus, /quit
        // or "ask" must still be handled then.
        self.snap = self.engine.snapshot();
        self.drain_events(ctx);
        self.open_pending_ask(ctx);
        if self.frame_no == 0 && self.start_minimized && !self.shot.is_active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        self.frame_no += 1;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shot.tick(ui.ctx());
        let ctx = ui.ctx().clone();
        let p = self.themes.palette().clone();

        self.snap = self.engine.snapshot();
        self.poll_folder_pick();
        self.prune_selection();
        self.handle_shortcuts(&ctx);

        let bar_margin = egui::Margin::symmetric(10, 4);
        egui::Panel::top("toolbar")
            .exact_size(62.0)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(bar_margin))
            .show(ui, |ui| self.toolbar(ui, &p));
        egui::Panel::bottom("statusbar")
            .exact_size(30.0)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(12, 2)))
            .show(ui, |ui| self.status_bar(ui, &p));
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(214.0)
            .size_range(180.0..=320.0)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(10, 10)))
            .show(ui, |ui| self.sidebar(ui, &p));
        if let Some(d) = self.focused_download() {
            egui::Panel::bottom("details")
                .resizable(true)
                .default_size(262.0)
                .size_range(190.0..=520.0)
                .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(14, 10)))
                .show(ui, |ui| self.details(ui, &p, &d));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin { left: 6, right: 6, top: 4, bottom: 0 }))
            .show(ui, |ui| self.table(ui, &p));

        self.new_download_dialog(&ctx, &p);
        self.batch_dialog(&ctx, &p);
        self.delete_dialog(&ctx, &p);
        self.properties_dialog(&ctx, &p);
        self.settings_dialog(&ctx, &p);

        let toast_actions = self.toasts.show(&ctx, &p, 42.0);
        self.handle_toast_actions(toast_actions, &ctx);
        self.handle_drops(&ctx, &p);
        self.apply_actions(&ctx);
        self.sync_settings();

        if self.folder_pick.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.sync_settings();
        self.engine.shutdown(Duration::from_secs(5));
    }
}
