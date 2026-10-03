//! Data types shared by the engine, the UI and persistence.

use crate::category::Category;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub type Id = u64;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Connecting,
    Downloading,
    #[default]
    Paused,
    Completed,
    Failed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Queued => "Queued",
            Status::Connecting => "Connecting",
            Status::Downloading => "Downloading",
            Status::Paused => "Paused",
            Status::Completed => "Completed",
            Status::Failed => "Failed",
        }
    }

    /// Name used by the local API.
    pub fn api_name(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Connecting => "connecting",
            Status::Downloading => "downloading",
            Status::Paused => "paused",
            Status::Completed => "completed",
            Status::Failed => "failed",
        }
    }

    /// Connecting or downloading.
    pub fn is_active(self) -> bool {
        matches!(self, Status::Connecting | Status::Downloading)
    }

    /// Can be (re)started with "Resume".
    pub fn can_resume(self) -> bool {
        matches!(self, Status::Paused | Status::Failed | Status::Queued)
    }
}

/// A byte range `[start, end)` of the file; `pos` is the next byte to fetch.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub pos: u64,
}

impl Segment {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end, pos: start }
    }
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.pos)
    }
    pub fn downloaded(&self) -> u64 {
        self.pos.saturating_sub(self.start)
    }
    pub fn is_done(&self) -> bool {
        self.pos >= self.end
    }
}

/// Browser context forwarded with every request.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct RequestInfo {
    pub referrer: Option<String>,
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
    pub headers: BTreeMap<String, String>,
}

/// Live state of one connection (runtime only).
#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connecting,
    Receiving,
    Retrying { attempt: u32, max: u32, error: String },
    /// Not started yet.
    Waiting,
    /// No work left for this connection.
    Idle,
    Failed(String),
}

impl ConnState {
    pub fn label(&self) -> String {
        match self {
            ConnState::Connecting => "Connecting".into(),
            ConnState::Receiving => "Receiving".into(),
            ConnState::Retrying { attempt, max, .. } => format!("Retry {attempt}/{max}"),
            ConnState::Waiting => "Waiting".into(),
            ConnState::Idle => "Finished".into(),
            ConnState::Failed(_) => "Failed".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConnInfo {
    pub id: usize,
    /// Current range `[start, end)`.
    pub start: u64,
    pub end: u64,
    pub pos: u64,
    pub speed: f64,
    pub received: u64,
    pub state: ConnState,
}

/// Runtime-only fields (never persisted).
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    /// Smoothed speed in bytes/s.
    pub speed: f64,
    /// Seconds remaining.
    pub eta: Option<f64>,
    pub conns: Vec<ConnInfo>,
    /// Speed samples, one per second (bytes/s), newest last.
    pub history: Vec<f32>,
    /// SHA-256 progress (0..1) while hashing.
    pub hashing: Option<f32>,
    /// Started by the queue (vs. manually) – "Stop queue" pauses these.
    pub from_queue: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Download {
    pub id: Id,
    pub url: String,
    /// URL after redirects (used for the actual transfer).
    pub final_url: Option<String>,
    pub file_name: String,
    /// The name was chosen by the user / extension; don't replace it after probing.
    pub name_locked: bool,
    /// Folder chosen by the user.
    pub save_dir: PathBuf,
    /// Resolved folder (incl. category sub-folder) once the download started.
    pub target_dir: Option<PathBuf>,
    pub total_size: Option<u64>,
    pub downloaded: u64,
    /// `None` until probed.
    pub resumable: Option<bool>,
    pub status: Status,
    pub error: Option<String>,
    /// Informational note shown in the details (e.g. "server can't resume").
    pub note: Option<String>,
    pub category: Category,
    pub category_locked: bool,
    pub connections: u32,
    /// The connection count came from the defaults (not picked by hand), so the
    /// small-file rule may lower it once the size is known.
    pub connections_auto: bool,
    /// Per-download limit in bytes/s.
    pub speed_limit: Option<u64>,
    pub segments: Vec<Segment>,
    pub added_at: u64,
    pub completed_at: Option<u64>,
    pub request: RequestInfo,
    pub mime: Option<String>,
    pub sha256: Option<String>,
    pub source: Option<String>,
    pub page_title: Option<String>,
    #[serde(skip)]
    pub rt: Runtime,
}

impl Default for Download {
    fn default() -> Self {
        Self {
            id: 0,
            url: String::new(),
            final_url: None,
            file_name: crate::filename::DEFAULT_NAME.to_owned(),
            name_locked: false,
            save_dir: PathBuf::new(),
            target_dir: None,
            total_size: None,
            downloaded: 0,
            resumable: None,
            status: Status::Paused,
            error: None,
            note: None,
            category: Category::Other,
            category_locked: false,
            connections: 8,
            connections_auto: false,
            speed_limit: None,
            segments: Vec::new(),
            added_at: 0,
            completed_at: None,
            request: RequestInfo::default(),
            mime: None,
            sha256: None,
            source: None,
            page_title: None,
            rt: Runtime::default(),
        }
    }
}

impl Download {
    /// Folder the file lives (or will live) in.
    pub fn dir(&self) -> PathBuf {
        self.target_dir.clone().unwrap_or_else(|| self.save_dir.clone())
    }
    /// Final path of the file.
    pub fn path(&self) -> PathBuf {
        self.dir().join(&self.file_name)
    }
    /// `<name>.zdm` temp path.
    pub fn temp_path(&self) -> PathBuf {
        crate::filename::temp_path(&self.path())
    }
    /// 0..1, `None` when the size is unknown.
    pub fn progress(&self) -> Option<f32> {
        if self.status == Status::Completed {
            return Some(1.0);
        }
        match self.total_size {
            Some(0) => Some(0.0),
            Some(t) => Some((self.downloaded as f64 / t as f64).clamp(0.0, 1.0) as f32),
            None => None,
        }
    }
    /// Remaining bytes if known.
    pub fn remaining(&self) -> Option<u64> {
        self.total_size.map(|t| t.saturating_sub(self.downloaded))
    }
}

/// How a newly added download should start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMode {
    /// Start right away (ignores the concurrency limit, like IDM's "Start download").
    Now,
    /// Append to the queue.
    Queue,
    /// Add paused.
    Paused,
}

/// Everything needed to add a download.
#[derive(Clone, Debug)]
pub struct NewDownload {
    pub url: String,
    pub file_name: Option<String>,
    pub save_dir: Option<PathBuf>,
    pub category: Option<Category>,
    pub connections: Option<u32>,
    pub request: RequestInfo,
    pub start: StartMode,
    pub size_hint: Option<u64>,
    pub mime: Option<String>,
    pub source: Option<String>,
    pub page_title: Option<String>,
}

impl NewDownload {
    pub fn new(url: impl Into<String>, start: StartMode) -> Self {
        Self {
            url: url.into(),
            file_name: None,
            save_dir: None,
            category: None,
            connections: None,
            request: RequestInfo::default(),
            start,
            size_hint: None,
            mime: None,
            source: None,
            page_title: None,
        }
    }
}

/// Result of probing a URL.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeInfo {
    pub final_url: String,
    pub total_size: Option<u64>,
    pub resumable: bool,
    pub file_name: String,
    pub mime: Option<String>,
}

/// A download request coming from a browser extension (`POST /download`)
/// or the command line; shown in the "New download" dialog in `ask` mode.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DownloadRequest {
    pub url: String,
    pub filename: Option<String>,
    pub referrer: Option<String>,
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
    pub size: Option<u64>,
    pub mime: Option<String>,
    pub page_title: Option<String>,
    pub source: Option<String>,
    pub mode: Option<String>,
    /// Local save folder (only accepted from callers without an `Origin`).
    pub save_dir: Option<PathBuf>,
}

impl DownloadRequest {
    pub fn request_info(&self) -> RequestInfo {
        RequestInfo {
            referrer: self.referrer.clone().filter(|s| !s.is_empty()),
            cookies: self.cookies.clone().filter(|s| !s.is_empty()),
            user_agent: self.user_agent.clone().filter(|s| !s.is_empty()),
            headers: self.headers.clone().unwrap_or_default(),
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct BatchItem {
    pub url: String,
    pub filename: Option<String>,
}

/// `POST /batch` body.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct BatchRequest {
    pub items: Vec<BatchItem>,
    pub referrer: Option<String>,
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
    pub page_title: Option<String>,
    pub source: Option<String>,
}

impl BatchRequest {
    pub fn request_info(&self) -> RequestInfo {
        RequestInfo {
            referrer: self.referrer.clone().filter(|s| !s.is_empty()),
            cookies: self.cookies.clone().filter(|s| !s.is_empty()),
            user_agent: self.user_agent.clone().filter(|s| !s.is_empty()),
            headers: BTreeMap::new(),
        }
    }
}

/// Events for the UI thread (drained every frame).
#[derive(Clone, Debug)]
pub enum UiEvent {
    Finished { id: Id, name: String, path: PathBuf },
    Failed { id: Id, name: String, error: String },
    /// Show the "New download" dialog pre-filled (API `mode: ask`, CLI).
    Ask(DownloadRequest),
    /// Show the batch dialog.
    Batch(BatchRequest),
    /// Restore + focus the window.
    Focus,
    /// Save and exit.
    Quit,
    /// Result of [`crate::engine::Engine::probe`].
    Probe { token: u64, result: Result<ProbeInfo, String> },
    /// A downloadable URL was copied to the clipboard.
    Clipboard(String),
    /// SHA-256 finished.
    Hashed { id: Id, name: String, result: Result<String, String> },
    /// Generic message for a toast.
    Info(String),
}
