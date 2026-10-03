//! The download engine.
//!
//! A tokio runtime runs on its own thread. The UI (or the API server) talks
//! to it through [`Engine`]: commands go over an unbounded channel, and the
//! engine publishes an immutable [`Snapshot`] (behind `Arc<Mutex<Arc<_>>>`)
//! after every change and ~4× per second while downloads run, calling the
//! notifier (`egui::Context::request_repaint`) so the UI redraws. Nothing in
//! here ever blocks the UI thread.

pub mod http;
pub mod limiter;
pub mod model;
pub mod segments;
pub mod store;
mod worker;

pub use model::*;

use crate::category::Category;
use crate::filename;
use crate::settings::Settings;
use limiter::RateLimiter;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use worker::{CancelToken, JobCtx, JobMeta, JobShared, JobSpec, Outcome, WorkerEvent};

/// Called whenever the snapshot changes (e.g. `ctx.request_repaint()`).
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Seconds of speed history kept for sparklines.
pub const HISTORY_LEN: usize = 60;
const TICK: Duration = Duration::from_millis(250);
const SAVE_DEBOUNCE: Duration = Duration::from_secs(2);

/// Commands understood by the engine.
#[derive(Debug)]
pub enum Command {
    Add { id: Id, new: Box<NewDownload> },
    Resume(Vec<Id>),
    Pause(Vec<Id>),
    PauseAll,
    Restart(Vec<Id>),
    Remove { ids: Vec<Id>, delete_files: bool },
    MoveUp(Vec<Id>),
    MoveDown(Vec<Id>),
    MoveTop(Vec<Id>),
    MoveBottom(Vec<Id>),
    StartQueue,
    StopQueue,
    SetSettings(Box<Settings>),
    /// Per-download limit in bytes/s (`None` = unlimited).
    SetItemLimit(Id, Option<u64>),
    SetConnections(Id, u32),
    Hash(Id),
    Probe { token: u64, url: String, request: RequestInfo },
    SaveNow,
    Shutdown(std::sync::mpsc::Sender<()>),
}

/// Immutable view of the engine state for the UI and the API.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Queue order.
    pub downloads: Vec<Download>,
    pub total_speed: f64,
    /// Total speed, one sample per second, newest last.
    pub speed_history: Vec<f32>,
    pub queue_running: bool,
    pub settings: Settings,
    pub revision: u64,
    /// Set if the HTTP client could not be created.
    pub client_error: Option<String>,
}

impl Snapshot {
    pub fn get(&self, id: Id) -> Option<&Download> {
        self.downloads.iter().find(|d| d.id == id)
    }
    pub fn count(&self, f: impl Fn(&Download) -> bool) -> usize {
        self.downloads.iter().filter(|d| f(d)).count()
    }
}

/// Startup configuration.
pub struct EngineConfig {
    pub settings: Settings,
    pub downloads: Vec<Download>,
    /// Where `settings.json` / `downloads.json` live; `None` disables persistence.
    pub data_dir: Option<PathBuf>,
    /// Demo mode: fake animated downloads, no network, no persistence.
    pub demo: bool,
}

impl EngineConfig {
    /// Loads settings and downloads from `dir`.
    pub fn load(dir: &Path) -> Self {
        Self {
            settings: store::load_settings(dir),
            downloads: store::load_downloads(dir),
            data_dir: Some(dir.to_path_buf()),
            demo: false,
        }
    }
}

/// Handle to the engine; cheap to clone and usable from any thread.
#[derive(Clone)]
pub struct Engine {
    tx: mpsc::UnboundedSender<Command>,
    snapshot: Arc<Mutex<Arc<Snapshot>>>,
    next_id: Arc<AtomicU64>,
    next_token: Arc<AtomicU64>,
}

impl Engine {
    /// Starts the runtime thread.
    pub fn start(config: EngineConfig, events: std::sync::mpsc::Sender<UiEvent>, notify: Notifier) -> io::Result<Engine> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("zdm-engine")
            .enable_all()
            .build()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let next = config.downloads.iter().map(|d| d.id).max().unwrap_or(0) + 1;
        let snapshot = Arc::new(Mutex::new(Arc::new(Snapshot {
            downloads: config.downloads.clone(),
            queue_running: true,
            settings: config.settings.clone(),
            ..Default::default()
        })));
        let engine = Engine {
            tx,
            snapshot: snapshot.clone(),
            next_id: Arc::new(AtomicU64::new(next)),
            next_token: Arc::new(AtomicU64::new(1)),
        };
        std::thread::Builder::new()
            .name("zdm-engine-main".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let (actor, worker_rx) = Actor::new(config, events, notify, snapshot);
                    actor.run(rx, worker_rx).await;
                });
            })?;
        Ok(engine)
    }

    /// Sends a command; returns `false` if the engine has stopped.
    pub fn send(&self, cmd: Command) -> bool {
        self.tx.send(cmd).is_ok()
    }

    /// The latest published state.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Adds a download and returns its id right away.
    pub fn add(&self, new: NewDownload) -> Id {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.send(Command::Add { id, new: Box::new(new) });
        id
    }

    /// Starts an asynchronous probe; the answer arrives as [`UiEvent::Probe`]
    /// carrying the returned token.
    pub fn probe(&self, url: &str, request: RequestInfo) -> u64 {
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        self.send(Command::Probe { token, url: url.to_owned(), request });
        token
    }

    /// Stops all transfers, saves and waits (bounded) for the engine to finish.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        if !self.send(Command::Shutdown(tx)) {
            return true;
        }
        rx.recv_timeout(timeout).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

/// What to do once a cancelled job has actually stopped.
enum AfterStop {
    /// Natural end (completed / failed).
    Nothing,
    Pause,
    Requeue,
    Remove { item: Box<Download>, delete_final: bool },
    Restart,
    Start { from_queue: bool },
    Shutdown,
}

struct JobHandle {
    shared: Arc<JobShared>,
    cancel: CancelToken,
    local: Arc<RateLimiter>,
    after: AfterStop,
    last_received: u64,
    conn_last: Vec<u64>,
    conn_speed: Vec<f64>,
    sec_bytes: u64,
    sec_time: f64,
}

impl JobHandle {
    fn stop(&mut self, after: AfterStop) {
        self.after = after;
        self.cancel.cancel();
    }
    fn stopping(&self) -> bool {
        !matches!(self.after, AfterStop::Nothing)
    }
}

struct Actor {
    settings: Settings,
    downloads: Vec<Download>,
    jobs: HashMap<Id, JobHandle>,
    queue_running: bool,
    client: Result<reqwest::Client, String>,
    client_timeout: u64,
    global: Arc<RateLimiter>,
    events: std::sync::mpsc::Sender<UiEvent>,
    notify: Notifier,
    snapshot: Arc<Mutex<Arc<Snapshot>>>,
    data_dir: Option<PathBuf>,
    worker_tx: mpsc::UnboundedSender<WorkerEvent>,
    dirty: bool,
    settings_dirty: bool,
    last_save: Instant,
    changed: bool,
    revision: u64,
    last_tick: Instant,
    total_speed: f64,
    history: VecDeque<f32>,
    sec_bytes: u64,
    sec_time: f64,
    demo: Option<crate::demo::DemoSim>,
    resume_on_start: Vec<Id>,
}

fn push_history(h: &mut Vec<f32>, v: f32) {
    h.push(v);
    if h.len() > HISTORY_LEN {
        let extra = h.len() - HISTORY_LEN;
        h.drain(..extra);
    }
}

fn build_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    http::build_client(Duration::from_secs(timeout_secs)).map_err(|e| format!("Could not initialise networking: {e}"))
}

impl Actor {
    fn new(
        config: EngineConfig,
        events: std::sync::mpsc::Sender<UiEvent>,
        notify: Notifier,
        snapshot: Arc<Mutex<Arc<Snapshot>>>,
    ) -> (Self, mpsc::UnboundedReceiver<WorkerEvent>) {
        let (worker_tx, worker_rx) = mpsc::unbounded_channel();
        let settings = config.settings.normalized();
        let mut downloads = config.downloads;
        // Demo items keep their fake "downloading" state; real ones that were
        // running when the app closed become paused (or resume automatically).
        let (demo, resume_on_start) = if config.demo {
            (Some(crate::demo::DemoSim::new(&downloads)), Vec::new())
        } else {
            let was_active = store::normalize_loaded(&mut downloads);
            (None, if settings.auto_resume { was_active } else { Vec::new() })
        };
        let mut history = VecDeque::with_capacity(HISTORY_LEN + 1);
        if demo.is_some() {
            // Pre-fill the global graph from the demo items' histories.
            for i in 0..HISTORY_LEN {
                let sum: f32 = downloads
                    .iter()
                    .filter(|d| d.status.is_active())
                    .map(|d| d.rt.history.get(i).copied().unwrap_or(0.0))
                    .sum();
                history.push_back(sum);
            }
        }
        let actor = Actor {
            client: build_client(settings.timeout_secs),
            client_timeout: settings.timeout_secs,
            global: Arc::new(RateLimiter::new(settings.global_limit_bps())),
            settings,
            downloads,
            jobs: HashMap::new(),
            queue_running: true,
            events,
            notify,
            snapshot,
            data_dir: config.data_dir,
            worker_tx,
            dirty: false,
            settings_dirty: false,
            last_save: Instant::now(),
            changed: true,
            revision: 0,
            last_tick: Instant::now(),
            total_speed: 0.0,
            history,
            sec_bytes: 0,
            sec_time: 0.0,
            demo,
            resume_on_start,
        };
        (actor, worker_rx)
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Command>, mut worker_rx: mpsc::UnboundedReceiver<WorkerEvent>) {
        for id in std::mem::take(&mut self.resume_on_start) {
            self.start_job(id, false);
        }
        self.schedule();
        self.publish();
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Command::Shutdown(reply)) => {
                        self.shutdown(&mut worker_rx).await;
                        let _ = reply.send(());
                        break;
                    }
                    Some(cmd) => self.handle(cmd),
                    None => {
                        self.shutdown(&mut worker_rx).await;
                        break;
                    }
                },
                Some(ev) = worker_rx.recv() => self.on_worker(ev),
                _ = tick.tick() => self.on_tick(),
            }
            if self.changed {
                self.publish();
            }
        }
    }

    fn publish(&mut self) {
        self.revision += 1;
        let snap = Snapshot {
            downloads: self.downloads.clone(),
            total_speed: self.total_speed,
            speed_history: self.history.iter().copied().collect(),
            queue_running: self.queue_running,
            settings: self.settings.clone(),
            revision: self.revision,
            client_error: self.client.as_ref().err().cloned(),
        };
        if let Ok(mut s) = self.snapshot.lock() {
            *s = Arc::new(snap);
        }
        self.changed = false;
        (self.notify)();
    }

    fn ui_event(&self, ev: UiEvent) {
        let _ = self.events.send(ev);
        (self.notify)();
    }

    fn mark(&mut self) {
        self.dirty = true;
        self.changed = true;
    }

    fn find(&mut self, id: Id) -> Option<&mut Download> {
        self.downloads.iter_mut().find(|d| d.id == id)
    }

    // -- commands ----------------------------------------------------------

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Add { id, new } => self.add(id, *new),
            Command::Resume(ids) => {
                for id in ids {
                    let ok = self.find(id).is_some_and(|d| d.status.can_resume() || d.status.is_active());
                    if ok {
                        self.start_job(id, false);
                    }
                }
            }
            Command::Pause(ids) => {
                for id in ids {
                    self.pause(id);
                }
            }
            Command::PauseAll => {
                let ids: Vec<Id> = self
                    .downloads
                    .iter()
                    .filter(|d| d.status.is_active() || d.status == Status::Queued)
                    .map(|d| d.id)
                    .collect();
                for id in ids {
                    self.pause(id);
                }
            }
            Command::Restart(ids) => {
                for id in ids {
                    if let Some(job) = self.jobs.get_mut(&id) {
                        job.stop(AfterStop::Restart);
                        if let Some(d) = self.find(id) {
                            d.status = Status::Connecting;
                        }
                    } else if self.find(id).is_some() {
                        self.reset_progress(id);
                        self.start_job(id, false);
                    }
                }
            }
            Command::Remove { ids, delete_files } => self.remove(&ids, delete_files),
            Command::MoveUp(ids) => self.move_items(&ids, Move::Up),
            Command::MoveDown(ids) => self.move_items(&ids, Move::Down),
            Command::MoveTop(ids) => self.move_items(&ids, Move::Top),
            Command::MoveBottom(ids) => self.move_items(&ids, Move::Bottom),
            Command::StartQueue => {
                self.queue_running = true;
            }
            Command::StopQueue => {
                self.queue_running = false;
                let ids: Vec<Id> = self
                    .downloads
                    .iter()
                    .filter(|d| d.rt.from_queue && d.status.is_active())
                    .map(|d| d.id)
                    .collect();
                for id in ids {
                    if let Some(job) = self.jobs.get_mut(&id) {
                        job.stop(AfterStop::Requeue);
                    }
                    if let Some(d) = self.find(id) {
                        d.status = Status::Queued;
                    }
                }
            }
            Command::SetSettings(s) => self.set_settings(*s),
            Command::SetItemLimit(id, limit) => {
                if let Some(d) = self.find(id) {
                    d.speed_limit = limit.filter(|&l| l > 0);
                }
                if let Some(job) = self.jobs.get(&id) {
                    job.local.set_rate(limit.unwrap_or(0));
                }
            }
            Command::SetConnections(id, n) => {
                if let Some(d) = self.find(id) {
                    d.connections = n.clamp(crate::settings::MIN_CONNECTIONS, crate::settings::MAX_CONNECTIONS);
                    // Picked by hand: the small-file rule no longer applies.
                    d.connections_auto = false;
                }
            }
            Command::Hash(id) => self.hash(id),
            Command::Probe { token, url, request } => self.probe(token, url, request),
            Command::SaveNow => {
                self.dirty = true;
                self.settings_dirty = true;
                self.maybe_save(true);
            }
            Command::Shutdown(_) => {}
        }
        self.schedule();
        self.mark();
    }

    fn add(&mut self, id: Id, new: NewDownload) {
        let url = new.url.trim().to_owned();
        let locked_name = new
            .file_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(filename::sanitize);
        let file_name = locked_name
            .clone()
            .or_else(|| filename::name_from_url(&url).map(|n| filename::sanitize(&n)))
            .map(|n| filename::ensure_extension(&n, new.mime.as_deref()))
            .unwrap_or_else(|| filename::DEFAULT_NAME.to_owned());
        let category = new.category.unwrap_or_else(|| Category::from_file_name(&file_name));
        let status = match new.start {
            StartMode::Queue => Status::Queued,
            StartMode::Now | StartMode::Paused => Status::Paused,
        };
        let dl = Download {
            id,
            url,
            file_name,
            name_locked: locked_name.is_some(),
            save_dir: new
                .save_dir
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| self.settings.download_dir.clone()),
            category,
            category_locked: new.category.is_some(),
            connections: new
                .connections
                .unwrap_or(self.settings.default_connections)
                .clamp(crate::settings::MIN_CONNECTIONS, crate::settings::MAX_CONNECTIONS),
            connections_auto: new.connections.is_none(),
            total_size: new.size_hint.filter(|&s| s > 0),
            status,
            added_at: crate::util::unix_now(),
            request: new.request,
            mime: new.mime,
            source: new.source,
            page_title: new.page_title,
            ..Default::default()
        };
        self.downloads.push(dl);
        if new.start == StartMode::Now {
            self.start_job(id, false);
        }
    }

    fn pause(&mut self, id: Id) {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.stop(AfterStop::Pause);
        }
        if let Some(d) = self.find(id)
            && (d.status.is_active() || d.status == Status::Queued)
        {
            d.status = Status::Paused;
            d.rt.speed = 0.0;
            d.rt.eta = None;
        }
    }

    fn remove(&mut self, ids: &[Id], delete_files: bool) {
        for &id in ids {
            let Some(pos) = self.downloads.iter().position(|d| d.id == id) else {
                continue;
            };
            let dl = self.downloads.remove(pos);
            if let Some(job) = self.jobs.get_mut(&id) {
                job.stop(AfterStop::Remove {
                    item: Box::new(dl),
                    delete_final: delete_files,
                });
            } else {
                delete_files_of(&dl, delete_files);
            }
        }
    }

    /// Forgets all progress and removes the partial file.
    fn reset_progress(&mut self, id: Id) {
        if let Some(d) = self.find(id) {
            if d.target_dir.is_some() {
                let _ = std::fs::remove_file(d.temp_path());
            }
            d.target_dir = None;
            d.segments.clear();
            d.downloaded = 0;
            d.completed_at = None;
            d.sha256 = None;
            d.error = None;
            d.note = None;
            d.rt.history.clear();
            if d.status == Status::Completed {
                d.status = Status::Paused;
            }
        }
    }

    fn move_items(&mut self, ids: &[Id], how: Move) {
        let selected = |d: &Download| ids.contains(&d.id);
        match how {
            Move::Up => {
                for i in 1..self.downloads.len() {
                    if selected(&self.downloads[i]) && !selected(&self.downloads[i - 1]) {
                        self.downloads.swap(i, i - 1);
                    }
                }
            }
            Move::Down => {
                for i in (0..self.downloads.len().saturating_sub(1)).rev() {
                    if selected(&self.downloads[i]) && !selected(&self.downloads[i + 1]) {
                        self.downloads.swap(i, i + 1);
                    }
                }
            }
            Move::Top | Move::Bottom => {
                let (mut sel, rest): (Vec<Download>, Vec<Download>) =
                    std::mem::take(&mut self.downloads).into_iter().partition(|d| selected(d));
                self.downloads = if how == Move::Top {
                    sel.extend(rest);
                    sel
                } else {
                    let mut r = rest;
                    r.extend(sel);
                    r
                };
            }
        }
    }

    fn set_settings(&mut self, s: Settings) {
        let s = s.normalized();
        self.global.set_rate(s.global_limit_bps());
        if s.timeout_secs != self.client_timeout {
            self.client = build_client(s.timeout_secs);
            self.client_timeout = s.timeout_secs;
        }
        if s != self.settings {
            self.settings = s;
            self.settings_dirty = true;
        }
    }

    fn probe(&mut self, token: u64, url: String, request: RequestInfo) {
        let events = self.events.clone();
        let notify = self.notify.clone();
        let client = self.client.clone();
        let ua = self.settings.user_agent.clone();
        let timeout = Duration::from_secs(self.settings.timeout_secs.min(20));
        if self.demo.is_some() {
            // Demo mode never touches the network.
            let name = filename::name_from_url(&url).map_or_else(|| "demo-file.zip".into(), |n| filename::sanitize(&n));
            let _ = events.send(UiEvent::Probe {
                token,
                result: Ok(ProbeInfo {
                    final_url: url,
                    total_size: Some(734_003_200),
                    resumable: true,
                    file_name: name,
                    mime: None,
                }),
            });
            notify();
            return;
        }
        tokio::spawn(async move {
            let result = match client {
                Ok(c) => http::probe(&c, &url, &request, &ua, timeout)
                    .await
                    .map_err(|e| e.message().to_owned()),
                Err(e) => Err(e),
            };
            let _ = events.send(UiEvent::Probe { token, result });
            notify();
        });
    }

    fn hash(&mut self, id: Id) {
        let Some(d) = self.find(id) else { return };
        if d.status != Status::Completed || d.rt.hashing.is_some() {
            return;
        }
        d.rt.hashing = Some(0.0);
        let path = d.path();
        let tx = self.worker_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = sha256_file(&path, |fraction| {
                let _ = tx.send(WorkerEvent::HashProgress { id, fraction });
            })
            .map_err(|e| format!("Could not read {}: {e}", path.display()));
            let _ = tx.send(WorkerEvent::Hashed { id, result });
        });
    }

    // -- scheduling ------------------------------------------------------------

    /// Starts queued downloads (FIFO, list order) while slots are free.
    fn schedule(&mut self) {
        if !self.queue_running {
            return;
        }
        loop {
            let active = self.downloads.iter().filter(|d| d.status.is_active()).count();
            if active >= self.settings.max_concurrent as usize {
                break;
            }
            let next = self
                .downloads
                .iter()
                .find(|d| d.status == Status::Queued && !self.jobs.contains_key(&d.id))
                .map(|d| d.id);
            match next {
                Some(id) => self.start_job(id, true),
                None => break,
            }
        }
    }

    fn start_job(&mut self, id: Id, from_queue: bool) {
        if let Some(job) = self.jobs.get_mut(&id) {
            // Still winding down: start again once it has stopped.
            if job.stopping() {
                job.after = AfterStop::Start { from_queue };
                if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                    d.status = Status::Connecting;
                }
            }
            return;
        }
        let settings = self.settings.clone();
        let client = self.client.clone();
        let demo = self.demo.is_some();
        let global = self.global.clone();
        let worker_tx = self.worker_tx.clone();
        let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) else { return };
        if d.status == Status::Completed {
            return;
        }
        d.error = None;
        d.rt.from_queue = from_queue;
        d.rt.conns.clear();
        if demo {
            d.status = Status::Downloading;
            return;
        }
        let client = match client {
            Ok(c) => c,
            Err(e) => {
                d.status = Status::Failed;
                d.error = Some(e);
                return;
            }
        };
        d.status = Status::Connecting;
        let connections = d.connections.clamp(crate::settings::MIN_CONNECTIONS, crate::settings::MAX_CONNECTIONS);
        let spec = JobSpec {
            id,
            url: d.url.clone(),
            request: d.request.clone(),
            user_agent: settings.user_agent.clone(),
            file_name: d.file_name.clone(),
            name_locked: d.name_locked,
            save_dir: d.save_dir.clone(),
            target_dir: d.target_dir.clone(),
            category: d.category,
            category_locked: d.category_locked,
            category_subfolders: settings.category_subfolders,
            total_size: d.total_size,
            resumable: d.resumable,
            segments: d.segments.clone(),
            connections,
            small_file: if d.connections_auto { settings.small_file_policy() } else { None },
            max_retries: settings.max_retries,
            timeout: Duration::from_secs(settings.timeout_secs),
        };
        let shared = Arc::new(JobShared::new(connections as usize));
        let cancel = CancelToken::default();
        let local = Arc::new(RateLimiter::new(d.speed_limit.unwrap_or(0)));
        let ctx = Arc::new(JobCtx {
            spec,
            client,
            shared: shared.clone(),
            cancel: cancel.clone(),
            global,
            local: local.clone(),
            events: worker_tx.clone(),
        });
        let tx = worker_tx;
        tokio::spawn(async move {
            let outcome = worker::run_job(ctx).await;
            let _ = tx.send(WorkerEvent::Done { id, outcome });
        });
        self.jobs.insert(
            id,
            JobHandle {
                shared,
                cancel,
                local,
                after: AfterStop::Nothing,
                last_received: 0,
                conn_last: vec![0; connections as usize],
                conn_speed: vec![0.0; connections as usize],
                sec_bytes: 0,
                sec_time: 0.0,
            },
        );
    }

    // -- worker events -----------------------------------------------------------

    fn on_worker(&mut self, ev: WorkerEvent) {
        match ev {
            WorkerEvent::Meta { id, meta } => self.on_meta(id, meta),
            WorkerEvent::Done { id, outcome } => self.on_done(id, outcome),
            WorkerEvent::HashProgress { id, fraction } => {
                if let Some(d) = self.find(id) {
                    d.rt.hashing = Some(fraction);
                }
                self.changed = true;
            }
            WorkerEvent::Hashed { id, result } => {
                let mut name = String::new();
                if let Some(d) = self.find(id) {
                    d.rt.hashing = None;
                    if let Ok(h) = &result {
                        d.sha256 = Some(h.clone());
                    }
                    name = d.file_name.clone();
                }
                self.ui_event(UiEvent::Hashed { id, name, result });
                self.mark();
            }
        }
        self.schedule();
    }

    fn on_meta(&mut self, id: Id, meta: JobMeta) {
        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            d.file_name = meta.file_name;
            d.target_dir = Some(meta.target_dir);
            d.category = meta.category;
            d.total_size = meta.total_size;
            d.resumable = Some(meta.resumable);
            d.connections = meta.connections;
            d.final_url = Some(meta.final_url).filter(|u| *u != d.url);
            if meta.mime.is_some() {
                d.mime = meta.mime;
            }
            if meta.restarted {
                d.downloaded = 0;
                d.segments.clear();
            }
            d.note = if !meta.resumable {
                Some(
                    "This server does not support resuming. Pausing or an interruption restarts \
                     the download from the beginning."
                        .into(),
                )
            } else if meta.restarted {
                Some("The file changed on the server or the partial file was missing, so the download restarted.".into())
            } else {
                None
            };
            self.mark();
        } else if let Some(job) = self.jobs.get_mut(&id)
            && let AfterStop::Remove { item, .. } = &mut job.after
        {
            item.file_name = meta.file_name;
            item.target_dir = Some(meta.target_dir);
        }
    }

    fn on_done(&mut self, id: Id, outcome: Outcome) {
        let Some(job) = self.jobs.remove(&id) else { return };
        let segs = job.shared.segments();
        let single = job.shared.single.load(Ordering::SeqCst);
        let stream_pos = job.shared.stream_pos.load(Ordering::SeqCst);
        if let AfterStop::Remove { item, delete_final } = job.after {
            let mut item = *item;
            if let Outcome::Completed { file_name, .. } = &outcome {
                item.file_name = file_name.clone();
                item.status = Status::Completed;
            }
            delete_files_of(&item, delete_final);
            self.mark();
            return;
        }
        let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) else { return };
        // Final progress.
        if !segs.is_empty() && !single {
            d.downloaded = segments::downloaded(&segs);
            d.segments = segs;
        } else if single {
            // Non-resumable progress can't be kept.
            d.downloaded = if matches!(outcome, Outcome::Completed { .. }) { stream_pos } else { 0 };
            d.segments.clear();
        }
        d.rt.speed = 0.0;
        d.rt.eta = None;
        d.rt.conns.clear();
        let mut ui = None;
        match outcome {
            Outcome::Completed { file_name, path, size } => {
                d.status = Status::Completed;
                d.file_name = file_name;
                d.total_size = Some(size);
                d.downloaded = size;
                d.segments.clear();
                d.completed_at = Some(crate::util::unix_now());
                d.error = None;
                ui = Some(UiEvent::Finished { id, name: d.file_name.clone(), path });
            }
            Outcome::Stopped | Outcome::Failed(_) if job.stopping() => {
                match job.after {
                    AfterStop::Pause | AfterStop::Nothing => d.status = Status::Paused,
                    AfterStop::Requeue => d.status = Status::Queued,
                    AfterStop::Shutdown => {} // keep "active" so the next launch can auto-resume
                    AfterStop::Restart => {
                        self.reset_progress(id);
                        self.start_job(id, false);
                    }
                    AfterStop::Start { from_queue } => {
                        if let Some(d) = self.find(id) {
                            d.status = Status::Paused;
                        }
                        self.start_job(id, from_queue);
                    }
                    AfterStop::Remove { .. } => {}
                }
            }
            Outcome::Stopped => d.status = Status::Paused,
            Outcome::Failed(e) => {
                d.status = Status::Failed;
                d.error = Some(e.clone());
                ui = Some(UiEvent::Failed { id, name: d.file_name.clone(), error: e });
            }
        }
        if let Some(ev) = ui {
            self.ui_event(ev);
        }
        self.mark();
    }

    // -- periodic work -------------------------------------------------------------

    fn on_tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().max(0.001);
        self.last_tick = now;
        let mut bytes = 0u64;
        for d in &mut self.downloads {
            if let Some(job) = self.jobs.get_mut(&d.id) {
                bytes += sync_job(d, job, dt);
            }
        }
        if let Some(sim) = &mut self.demo {
            bytes += sim.tick(&mut self.downloads, dt);
        }
        let inst = bytes as f64 / dt;
        self.total_speed = if self.total_speed <= 0.0 { inst } else { self.total_speed * 0.75 + inst * 0.25 };
        if self.total_speed < 1.0 {
            self.total_speed = 0.0;
        }
        self.sec_bytes += bytes;
        self.sec_time += dt;
        if self.sec_time >= 1.0 {
            let any_active = self.downloads.iter().any(|d| d.status.is_active());
            if any_active || self.history.iter().any(|&v| v > 0.0) {
                self.history.push_back((self.sec_bytes as f64 / self.sec_time) as f32);
                while self.history.len() > HISTORY_LEN {
                    self.history.pop_front();
                }
                self.changed = true;
            }
            self.sec_bytes = 0;
            self.sec_time = 0.0;
        }
        if !self.jobs.is_empty() {
            self.dirty = true;
            self.changed = true;
        }
        if self.demo.is_some() || self.total_speed > 0.0 {
            self.changed = true;
        }
        self.maybe_save(false);
    }

    fn maybe_save(&mut self, force: bool) {
        let Some(dir) = self.data_dir.clone() else {
            self.dirty = false;
            self.settings_dirty = false;
            return;
        };
        if !force && self.last_save.elapsed() < SAVE_DEBOUNCE {
            return;
        }
        if self.dirty {
            if let Err(e) = store::save_downloads(&dir, &self.downloads) {
                eprintln!("could not save downloads: {e}");
            }
            self.dirty = false;
        }
        if self.settings_dirty {
            if let Err(e) = store::save_settings(&dir, &self.settings) {
                eprintln!("could not save settings: {e}");
            }
            self.settings_dirty = false;
        }
        self.last_save = Instant::now();
    }

    async fn shutdown(&mut self, worker_rx: &mut mpsc::UnboundedReceiver<WorkerEvent>) {
        for job in self.jobs.values_mut() {
            job.stop(AfterStop::Shutdown);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !self.jobs.is_empty() {
            match tokio::time::timeout_at(deadline, worker_rx.recv()).await {
                Ok(Some(ev)) => self.on_worker(ev),
                _ => break,
            }
        }
        // Anything still running: keep whatever progress it reported.
        for d in &mut self.downloads {
            if let Some(job) = self.jobs.get(&d.id) {
                let segs = job.shared.segments();
                if !segs.is_empty() && !job.shared.single.load(Ordering::SeqCst) {
                    d.downloaded = segments::downloaded(&segs);
                    d.segments = segs;
                }
            }
        }
        self.dirty = true;
        self.settings_dirty = true;
        self.maybe_save(true);
        self.publish();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Move {
    Up,
    Down,
    Top,
    Bottom,
}

/// Copies a running job's progress into its download; returns bytes received since the last tick.
fn sync_job(d: &mut Download, job: &mut JobHandle, dt: f64) -> u64 {
    let sh = &job.shared;
    let segs = sh.segments();
    if sh.single.load(Ordering::Relaxed) {
        d.downloaded = sh.stream_pos.load(Ordering::Relaxed);
        d.segments = segs.clone();
    } else if !segs.is_empty() {
        d.downloaded = segments::downloaded(&segs);
        d.segments = segs.clone();
    }
    if sh.phase.load(Ordering::Relaxed) == worker::PHASE_DOWNLOADING && d.status == Status::Connecting && !job.stopping() {
        d.status = Status::Downloading;
    }
    let received = sh.received.load(Ordering::Relaxed);
    let delta = received.saturating_sub(job.last_received);
    job.last_received = received;
    let inst = delta as f64 / dt;
    d.rt.speed = if d.rt.speed <= 0.0 { inst } else { d.rt.speed * 0.75 + inst * 0.25 };
    if d.rt.speed < 1.0 {
        d.rt.speed = 0.0;
    }
    d.rt.eta = d
        .remaining()
        .filter(|_| d.rt.speed > 1.0)
        .map(|r| r as f64 / d.rt.speed);
    job.sec_bytes += delta;
    job.sec_time += dt;
    if job.sec_time >= 1.0 {
        push_history(&mut d.rt.history, (job.sec_bytes as f64 / job.sec_time) as f32);
        job.sec_bytes = 0;
        job.sec_time = 0.0;
    }
    let states = sh.conn_state.lock().map(|s| s.clone()).unwrap_or_default();
    d.rt.conns = states
        .into_iter()
        .enumerate()
        .map(|(i, (state, slot))| {
            let bytes = sh.conn_bytes.get(i).map_or(0, |b| b.load(Ordering::Relaxed));
            let last = job.conn_last.get(i).copied().unwrap_or(0);
            if let Some(l) = job.conn_last.get_mut(i) {
                *l = bytes;
            }
            let inst = bytes.saturating_sub(last) as f64 / dt;
            let speed = job.conn_speed.get_mut(i).map_or(0.0, |s| {
                *s = *s * 0.7 + inst * 0.3;
                *s
            });
            let seg = slot.and_then(|s| segs.get(s)).copied();
            ConnInfo {
                id: i + 1,
                start: seg.map_or(0, |s| s.start),
                end: seg.map_or(0, |s| s.end),
                pos: seg.map_or(0, |s| s.pos),
                speed: if state == ConnState::Receiving { speed } else { 0.0 },
                received: bytes,
                state,
            }
        })
        .collect();
    delta
}

/// Deletes the partial file (always) and the finished file (if asked).
fn delete_files_of(d: &Download, delete_final: bool) {
    if d.target_dir.is_some() {
        let _ = std::fs::remove_file(d.temp_path());
    }
    if delete_final && d.status == Status::Completed {
        let _ = std::fs::remove_file(d.path());
    }
}

/// SHA-256 of a file as lowercase hex, reporting progress.
pub fn sha256_file(path: &Path, mut progress: impl FnMut(f32)) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let total = f.metadata().map(|m| m.len()).unwrap_or(0).max(1);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let mut last_report = 0.0f32;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        let frac = (done as f64 / total as f64) as f32;
        if frac - last_report >= 0.02 {
            last_report = frac;
            progress(frac.min(1.0));
        }
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_value() {
        let p = std::env::temp_dir().join(format!("zdm-sha-{}.txt", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        let h = sha256_file(&p, |_| {}).unwrap();
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = Vec::new();
        for i in 0..100 {
            push_history(&mut h, i as f32);
        }
        assert_eq!(h.len(), HISTORY_LEN);
        assert_eq!(h[HISTORY_LEN - 1], 99.0);
    }
}
