//! One running download: probe, pick the file, run the connections, finish.

use super::http::{self, HttpError};
use super::limiter::RateLimiter;
use super::model::{ConnState, Id, RequestInfo, Segment};
use super::segments::{self, MAX_WRITE, Slot};
use crate::category::Category;
use crate::filename;
use reqwest::header;
use reqwest::{Client, StatusCode};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinSet;

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

/// Cheap cloneable cancellation flag with an async wait.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<CancelInner>);

#[derive(Default)]
struct CancelInner {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }
    pub async fn cancelled(&self) {
        loop {
            // `notified()` registers before we check the flag, so no wake-up is lost.
            let n = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            n.await;
        }
    }
    /// Sleeps for `d`; returns `true` if cancelled meanwhile.
    pub async fn sleep(&self, d: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(d) => self.is_cancelled(),
            _ = self.cancelled() => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared state between a job and the engine
// ---------------------------------------------------------------------------

pub const PHASE_CONNECTING: u8 = 0;
pub const PHASE_DOWNLOADING: u8 = 1;

/// Progress shared between a running job (writer) and the engine tick (reader).
pub struct JobShared {
    pub slots: Mutex<Vec<Slot>>,
    /// Per-connection state and the slot it works on.
    pub conn_state: Mutex<Vec<(ConnState, Option<usize>)>>,
    /// Bytes received per connection (this run).
    pub conn_bytes: Vec<AtomicU64>,
    /// Bytes received in total (this run) – used for speed.
    pub received: AtomicU64,
    /// Bytes written in single-stream mode.
    pub stream_pos: AtomicU64,
    /// Single-stream mode (no ranges).
    pub single: AtomicBool,
    pub phase: AtomicU8,
}

impl JobShared {
    pub fn new(connections: usize) -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
            conn_state: Mutex::new(vec![(ConnState::Waiting, None); connections]),
            conn_bytes: (0..connections).map(|_| AtomicU64::new(0)).collect(),
            received: AtomicU64::new(0),
            stream_pos: AtomicU64::new(0),
            single: AtomicBool::new(false),
            phase: AtomicU8::new(PHASE_CONNECTING),
        }
    }

    /// Copy of the current segments.
    pub fn segments(&self) -> Vec<Segment> {
        self.slots
            .lock()
            .map(|s| s.iter().map(|x| x.seg).collect())
            .unwrap_or_default()
    }

    fn set_conn(&self, conn: usize, state: ConnState, slot: Option<usize>) {
        if let Ok(mut c) = self.conn_state.lock()
            && let Some(entry) = c.get_mut(conn)
        {
            *entry = (state, slot);
        }
    }

    fn slot(&self, idx: usize) -> Option<Segment> {
        self.slots.lock().ok().and_then(|s| s.get(idx).map(|x| x.seg))
    }

    fn release(&self, idx: usize) {
        if let Ok(mut s) = self.slots.lock()
            && let Some(slot) = s.get_mut(idx)
        {
            slot.active = false;
        }
    }

    fn advance(&self, idx: usize, n: u64) {
        if let Ok(mut s) = self.slots.lock()
            && let Some(slot) = s.get_mut(idx)
        {
            slot.seg.pos += n;
        }
    }
}

// ---------------------------------------------------------------------------
// Job description and results
// ---------------------------------------------------------------------------

/// Everything a job needs to know about its download (copied at start).
#[derive(Clone, Debug)]
pub struct JobSpec {
    pub id: Id,
    pub url: String,
    pub request: RequestInfo,
    pub user_agent: String,
    pub file_name: String,
    pub name_locked: bool,
    pub save_dir: PathBuf,
    pub target_dir: Option<PathBuf>,
    pub category: Category,
    pub category_locked: bool,
    pub category_subfolders: bool,
    pub total_size: Option<u64>,
    pub resumable: Option<bool>,
    pub segments: Vec<Segment>,
    pub connections: u32,
    /// Fewer connections for small files (`None`: rule off or count picked by hand).
    pub small_file: Option<segments::SmallFilePolicy>,
    pub max_retries: u32,
    pub timeout: Duration,
}

/// Learned after probing; the engine copies it into the download.
#[derive(Clone, Debug)]
pub struct JobMeta {
    pub file_name: String,
    pub target_dir: PathBuf,
    pub category: Category,
    pub total_size: Option<u64>,
    pub resumable: bool,
    pub final_url: String,
    pub mime: Option<String>,
    /// Existing progress could not be reused (size changed / no resume support).
    pub restarted: bool,
    /// Connections actually used (after the small-file rule).
    pub connections: u32,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Completed { file_name: String, path: PathBuf, size: u64 },
    Stopped,
    Failed(String),
}

pub enum WorkerEvent {
    Meta { id: Id, meta: JobMeta },
    Done { id: Id, outcome: Outcome },
    HashProgress { id: Id, fraction: f32 },
    Hashed { id: Id, result: Result<String, String> },
}

pub struct JobCtx {
    pub spec: JobSpec,
    pub client: Client,
    pub shared: Arc<JobShared>,
    pub cancel: CancelToken,
    pub global: Arc<RateLimiter>,
    pub local: Arc<RateLimiter>,
    pub events: mpsc::UnboundedSender<WorkerEvent>,
}

enum JobError {
    Cancelled,
    Failed(String),
}

enum FetchError {
    Cancelled,
    Fatal(String),
    Retry { error: String, progressed: bool },
}

impl From<HttpError> for FetchError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Retry(error) => FetchError::Retry { error, progressed: false },
            HttpError::Fatal(e) => FetchError::Fatal(e),
        }
    }
}

// ---------------------------------------------------------------------------
// File helpers
// ---------------------------------------------------------------------------

/// Positional write that doesn't disturb other connections.
pub fn write_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0usize;
        while done < buf.len() {
            let n = file.seek_write(&buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "wrote zero bytes"));
            }
            done += n;
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
}

/// Creates `<dir>/<name>.zdm` exclusively, adding " (n)" to the name until
/// neither the final file nor its temp file exists.
fn reserve_unique(dir: &Path, name: &str) -> io::Result<(String, File)> {
    let mut last_err = None;
    for _ in 0..50 {
        let candidate = filename::unique_name(name, |n| {
            let p = dir.join(n);
            p.exists() || filename::temp_path(&p).exists()
        });
        let temp = filename::temp_path(&dir.join(&candidate));
        match OpenOptions::new().read(true).write(true).create_new(true).open(&temp) {
            Ok(f) => return Ok((candidate, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("could not pick a file name")))
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

pub async fn run_job(ctx: Arc<JobCtx>) -> Outcome {
    match run_inner(&ctx).await {
        Ok(o) => o,
        Err(JobError::Cancelled) => Outcome::Stopped,
        Err(JobError::Failed(e)) => {
            if ctx.cancel.is_cancelled() {
                Outcome::Stopped
            } else {
                Outcome::Failed(e)
            }
        }
    }
}

async fn probe_with_retries(ctx: &JobCtx) -> Result<crate::engine::ProbeInfo, JobError> {
    let spec = &ctx.spec;
    let mut attempt = 0;
    loop {
        if ctx.cancel.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        ctx.shared.set_conn(0, ConnState::Connecting, None);
        let result = tokio::select! {
            r = http::probe(&ctx.client, &spec.url, &spec.request, &spec.user_agent, spec.timeout) => r,
            _ = ctx.cancel.cancelled() => return Err(JobError::Cancelled),
        };
        match result {
            Ok(p) => return Ok(p),
            Err(HttpError::Fatal(e)) => return Err(JobError::Failed(e)),
            Err(HttpError::Retry(e)) => {
                attempt += 1;
                if attempt > spec.max_retries {
                    return Err(JobError::Failed(e));
                }
                ctx.shared.set_conn(
                    0,
                    ConnState::Retrying { attempt, max: spec.max_retries, error: e },
                    None,
                );
                if ctx.cancel.sleep(http::backoff(attempt)).await {
                    return Err(JobError::Cancelled);
                }
            }
        }
    }
}

async fn run_inner(ctx: &Arc<JobCtx>) -> Result<Outcome, JobError> {
    let spec = &ctx.spec;
    let probe = probe_with_retries(ctx).await?;
    let total = probe.total_size;
    let resumable = probe.resumable && total.is_some();
    // Small files get fewer connections so servers don't take us for a bot.
    let connections = segments::effective_connections(spec.connections, total, spec.small_file);
    if let Ok(mut states) = ctx.shared.conn_state.lock() {
        states.truncate(connections.max(1) as usize);
    }

    // Can we continue the existing temp file?
    let old_temp = spec
        .target_dir
        .as_ref()
        .map(|d| filename::temp_path(&d.join(&spec.file_name)));
    let can_continue = resumable
        && spec.resumable == Some(true)
        && spec.total_size == total
        && !spec.segments.is_empty()
        && total.is_some_and(|t| segments::validate(&spec.segments, t))
        && old_temp.as_ref().is_some_and(|p| p.exists());
    let had_progress = spec.segments.iter().any(|s| s.downloaded() > 0);
    let restarted = had_progress && !can_continue;

    let (file_name, category, dir, file) = if can_continue {
        let dir = spec.target_dir.clone().unwrap_or_else(|| spec.save_dir.clone());
        let temp = old_temp.clone().unwrap_or_default();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temp)
            .map_err(|e| JobError::Failed(format!("Cannot open {}: {e}", temp.display())))?;
        (spec.file_name.clone(), spec.category, dir, file)
    } else {
        if let Some(old) = &old_temp {
            let _ = std::fs::remove_file(old);
        }
        let (name, category) = if spec.name_locked {
            (spec.file_name.clone(), spec.category)
        } else {
            let cat = if spec.category_locked {
                spec.category
            } else {
                Category::from_file_name(&probe.file_name)
            };
            (probe.file_name.clone(), cat)
        };
        let dir = if spec.category_subfolders {
            spec.save_dir.join(category.folder_name())
        } else {
            spec.save_dir.clone()
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| JobError::Failed(format!("Cannot create folder {}: {e}", dir.display())))?;
        let (name, file) = reserve_unique(&dir, &name)
            .map_err(|e| JobError::Failed(format!("Cannot create file in {}: {e}", dir.display())))?;
        if let Some(t) = total {
            // Preallocate so positional writes land in place.
            file.set_len(t).map_err(|e| {
                let _ = std::fs::remove_file(filename::temp_path(&dir.join(&name)));
                JobError::Failed(format!("Cannot reserve {t} bytes (disk full?): {e}"))
            })?;
        }
        (name, category, dir, file)
    };

    let _ = ctx.events.send(WorkerEvent::Meta {
        id: spec.id,
        meta: JobMeta {
            file_name: file_name.clone(),
            target_dir: dir.clone(),
            category,
            total_size: total,
            resumable,
            final_url: probe.final_url.clone(),
            mime: probe.mime.clone(),
            restarted,
            connections,
        },
    });

    let file = Arc::new(file);
    let url: Arc<str> = Arc::from(probe.final_url.as_str());
    let size = match total {
        Some(t) if resumable && t > 0 => {
            let segs = if can_continue {
                spec.segments.clone()
            } else {
                segments::initial_segments(t, connections)
            };
            if let Ok(mut slots) = ctx.shared.slots.lock() {
                *slots = segs.into_iter().map(|seg| Slot { seg, active: false }).collect();
            }
            ctx.shared.phase.store(PHASE_DOWNLOADING, Ordering::SeqCst);
            run_segmented(ctx, connections, url, file.clone()).await?;
            t
        }
        _ => {
            ctx.shared.single.store(true, Ordering::SeqCst);
            if let (Some(t), Ok(mut slots)) = (total, ctx.shared.slots.lock()) {
                *slots = vec![Slot { seg: Segment::new(0, t), active: true }];
            }
            ctx.shared.phase.store(PHASE_DOWNLOADING, Ordering::SeqCst);
            run_single(ctx, &url, &file, total).await?
        }
    };

    // Finish: flush, close, rename `<name>.zdm` → `<name>`.
    if let Err(e) = file.sync_all() {
        return Err(JobError::Failed(format!("Could not flush the file: {e}")));
    }
    drop(file);
    let temp = filename::temp_path(&dir.join(&file_name));
    let final_name = filename::unique_name(&file_name, |n| dir.join(n).exists());
    let final_path = dir.join(&final_name);
    std::fs::rename(&temp, &final_path)
        .map_err(|e| JobError::Failed(format!("Could not rename the finished file: {e}")))?;
    Ok(Outcome::Completed {
        file_name: final_name,
        path: final_path,
        size,
    })
}

// ---------------------------------------------------------------------------
// Segmented (multi-connection) transfer
// ---------------------------------------------------------------------------

async fn run_segmented(ctx: &Arc<JobCtx>, connections: u32, url: Arc<str>, file: Arc<File>) -> Result<(), JobError> {
    let mut set = JoinSet::new();
    for conn in 0..connections.max(1) as usize {
        let (ctx, url, file) = (ctx.clone(), url.clone(), file.clone());
        set.spawn(async move { connection_loop(&ctx, conn, &url, &file).await });
    }
    let mut last_error: Option<String> = None;
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Err(e)) => last_error = Some(e),
            Ok(Ok(())) => {}
            Err(e) => last_error = Some(format!("Connection task crashed: {e}")),
        }
    }
    if ctx.cancel.is_cancelled() {
        return Err(JobError::Cancelled);
    }
    let segs = ctx.shared.segments();
    if segments::all_done(&segs) {
        Ok(())
    } else {
        Err(JobError::Failed(
            last_error.unwrap_or_else(|| "Download incomplete".to_owned()),
        ))
    }
}

/// One connection: keep claiming work (dynamic segmentation) until none is left.
async fn connection_loop(ctx: &JobCtx, conn: usize, url: &str, file: &File) -> Result<(), String> {
    let max = ctx.spec.max_retries;
    loop {
        if ctx.cancel.is_cancelled() {
            return Ok(());
        }
        let claimed = ctx.shared.slots.lock().ok().and_then(|mut s| segments::claim(&mut s));
        let Some(idx) = claimed else {
            ctx.shared.set_conn(conn, ConnState::Idle, None);
            return Ok(());
        };
        let mut failures = 0u32;
        loop {
            match fetch_range(ctx, conn, idx, url, file).await {
                Ok(()) => {
                    ctx.shared.release(idx);
                    break;
                }
                Err(FetchError::Cancelled) => {
                    ctx.shared.release(idx);
                    return Ok(());
                }
                Err(FetchError::Fatal(e)) => {
                    ctx.shared.release(idx);
                    ctx.shared.set_conn(conn, ConnState::Failed(e.clone()), None);
                    return Err(e);
                }
                Err(FetchError::Retry { error, progressed }) => {
                    if progressed {
                        failures = 0;
                    }
                    failures += 1;
                    if failures > max {
                        ctx.shared.release(idx);
                        ctx.shared.set_conn(conn, ConnState::Failed(error.clone()), None);
                        return Err(error);
                    }
                    ctx.shared.set_conn(
                        conn,
                        ConnState::Retrying { attempt: failures, max, error },
                        Some(idx),
                    );
                    if ctx.cancel.sleep(http::backoff(failures)).await {
                        ctx.shared.release(idx);
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Downloads the remainder of slot `idx` with a `Range` request. Returns
/// `Ok` once the slot is complete (possibly early because it was split).
async fn fetch_range(ctx: &JobCtx, conn: usize, idx: usize, url: &str, file: &File) -> Result<(), FetchError> {
    let spec = &ctx.spec;
    let Some(seg) = ctx.shared.slot(idx) else {
        return Ok(());
    };
    if seg.is_done() {
        return Ok(());
    }
    ctx.shared.set_conn(conn, ConnState::Connecting, Some(idx));
    let req = ctx
        .client
        .get(url)
        .headers(http::request_headers(&spec.request, &spec.user_agent))
        .header(header::RANGE, format!("bytes={}-{}", seg.pos, seg.end - 1));
    let mut resp = tokio::select! {
        r = tokio::time::timeout(spec.timeout, req.send()) => match r {
            Err(_) => return Err(FetchError::Retry { error: "Timed out waiting for the server".into(), progressed: false }),
            Ok(Err(e)) => return Err(http::describe(&e).into()),
            Ok(Ok(r)) => r,
        },
        _ = ctx.cancel.cancelled() => return Err(FetchError::Cancelled),
    };
    let status = resp.status();
    if status == StatusCode::PARTIAL_CONTENT {
        let range = resp
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(http::parse_content_range);
        if let Some((start, _, _)) = range
            && start != seg.pos
        {
            return Err(FetchError::Retry {
                error: format!("Server sent bytes from {start} instead of {}", seg.pos),
                progressed: false,
            });
        }
    } else if status.is_success() {
        // The whole body from byte 0 is only usable if that's where we are.
        if seg.pos != 0 {
            return Err(FetchError::Fatal("The server stopped accepting range requests".into()));
        }
    } else {
        return Err(http::status_error(status).into());
    }

    ctx.shared.set_conn(conn, ConnState::Receiving, Some(idx));
    let mut progressed = false;
    loop {
        let chunk = tokio::select! {
            c = tokio::time::timeout(spec.timeout, resp.chunk()) => c,
            _ = ctx.cancel.cancelled() => return Err(FetchError::Cancelled),
        };
        let bytes = match chunk {
            Err(_) => return Err(FetchError::Retry { error: "Read timed out".into(), progressed }),
            Ok(Err(e)) => {
                return Err(FetchError::Retry { error: http::describe(&e).message().to_owned(), progressed });
            }
            Ok(Ok(None)) => {
                let done = ctx.shared.slot(idx).is_none_or(|s| s.is_done());
                return if done {
                    Ok(())
                } else {
                    Err(FetchError::Retry { error: "Connection closed early".into(), progressed })
                };
            }
            Ok(Ok(Some(b))) => b,
        };
        let mut data: &[u8] = &bytes;
        while !data.is_empty() {
            let piece = data.len().min(MAX_WRITE);
            let wait = ctx.global.reserve(piece).max(ctx.local.reserve(piece));
            if !wait.is_zero() && ctx.cancel.sleep(wait).await {
                return Err(FetchError::Cancelled);
            }
            // Re-read the end: another connection may have split this slot.
            let Some(seg) = ctx.shared.slot(idx) else {
                return Ok(());
            };
            if seg.is_done() {
                return Ok(());
            }
            let n = (piece as u64).min(seg.remaining()) as usize;
            write_at(file, &data[..n], seg.pos)
                .map_err(|e| FetchError::Fatal(format!("Disk write failed: {e}")))?;
            ctx.shared.advance(idx, n as u64);
            ctx.shared.received.fetch_add(n as u64, Ordering::Relaxed);
            if let Some(c) = ctx.shared.conn_bytes.get(conn) {
                c.fetch_add(n as u64, Ordering::Relaxed);
            }
            progressed = true;
            if n < piece {
                return Ok(()); // reached the (possibly moved) end
            }
            data = &data[n..];
        }
    }
}

// ---------------------------------------------------------------------------
// Single-stream transfer (no range support / unknown size)
// ---------------------------------------------------------------------------

/// Streams the whole body; every retry restarts from byte 0 because the
/// server can't resume. Returns the final size.
async fn run_single(ctx: &JobCtx, url: &str, file: &File, total: Option<u64>) -> Result<u64, JobError> {
    let max = ctx.spec.max_retries;
    let mut failures = 0u32;
    loop {
        if ctx.cancel.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        ctx.shared.stream_pos.store(0, Ordering::SeqCst);
        if let Ok(mut s) = ctx.shared.slots.lock()
            && let Some(slot) = s.first_mut()
        {
            slot.seg.pos = 0;
        }
        match fetch_stream(ctx, url, file, total).await {
            Ok(n) => {
                if total.is_none() {
                    file.set_len(n)
                        .map_err(|e| JobError::Failed(format!("Could not finalize the file: {e}")))?;
                }
                ctx.shared.set_conn(0, ConnState::Idle, None);
                return Ok(n);
            }
            Err(FetchError::Cancelled) => return Err(JobError::Cancelled),
            Err(FetchError::Fatal(e)) => {
                ctx.shared.set_conn(0, ConnState::Failed(e.clone()), None);
                return Err(JobError::Failed(e));
            }
            Err(FetchError::Retry { error, progressed }) => {
                if progressed {
                    failures = 0;
                }
                failures += 1;
                if failures > max {
                    ctx.shared.set_conn(0, ConnState::Failed(error.clone()), None);
                    return Err(JobError::Failed(error));
                }
                ctx.shared.set_conn(0, ConnState::Retrying { attempt: failures, max, error }, None);
                if ctx.cancel.sleep(http::backoff(failures)).await {
                    return Err(JobError::Cancelled);
                }
            }
        }
    }
}

async fn fetch_stream(ctx: &JobCtx, url: &str, file: &File, total: Option<u64>) -> Result<u64, FetchError> {
    let spec = &ctx.spec;
    ctx.shared.set_conn(0, ConnState::Connecting, Some(0));
    let req = ctx
        .client
        .get(url)
        .headers(http::request_headers(&spec.request, &spec.user_agent));
    let mut resp = tokio::select! {
        r = tokio::time::timeout(spec.timeout, req.send()) => match r {
            Err(_) => return Err(FetchError::Retry { error: "Timed out waiting for the server".into(), progressed: false }),
            Ok(Err(e)) => return Err(http::describe(&e).into()),
            Ok(Ok(r)) => r,
        },
        _ = ctx.cancel.cancelled() => return Err(FetchError::Cancelled),
    };
    if !resp.status().is_success() {
        return Err(http::status_error(resp.status()).into());
    }
    ctx.shared.set_conn(0, ConnState::Receiving, Some(0));
    let mut pos: u64 = 0;
    let mut progressed = false;
    loop {
        let chunk = tokio::select! {
            c = tokio::time::timeout(spec.timeout, resp.chunk()) => c,
            _ = ctx.cancel.cancelled() => return Err(FetchError::Cancelled),
        };
        let bytes = match chunk {
            Err(_) => return Err(FetchError::Retry { error: "Read timed out".into(), progressed }),
            Ok(Err(e)) => {
                return Err(FetchError::Retry { error: http::describe(&e).message().to_owned(), progressed });
            }
            Ok(Ok(None)) => {
                return match total {
                    Some(t) if pos < t => Err(FetchError::Retry { error: "Connection closed early".into(), progressed }),
                    _ => Ok(pos),
                };
            }
            Ok(Ok(Some(b))) => b,
        };
        let mut data: &[u8] = &bytes;
        if let Some(t) = total {
            // Ignore anything past the announced size.
            let room = t.saturating_sub(pos) as usize;
            data = &data[..data.len().min(room)];
        }
        while !data.is_empty() {
            let n = data.len().min(MAX_WRITE);
            let wait = ctx.global.reserve(n).max(ctx.local.reserve(n));
            if !wait.is_zero() && ctx.cancel.sleep(wait).await {
                return Err(FetchError::Cancelled);
            }
            write_at(file, &data[..n], pos).map_err(|e| FetchError::Fatal(format!("Disk write failed: {e}")))?;
            pos += n as u64;
            progressed = true;
            ctx.shared.stream_pos.store(pos, Ordering::SeqCst);
            ctx.shared.received.fetch_add(n as u64, Ordering::Relaxed);
            if let Some(c) = ctx.shared.conn_bytes.first() {
                c.fetch_add(n as u64, Ordering::Relaxed);
            }
            if let Ok(mut s) = ctx.shared.slots.lock()
                && let Some(slot) = s.first_mut()
            {
                slot.seg.pos = pos;
            }
            data = &data[n..];
        }
        if total.is_some_and(|t| pos >= t) {
            return Ok(pos);
        }
    }
}
