//! End-to-end engine tests against a local tiny_http server.
//!
//! * a server with `Range` support → multi-connection download with dynamic
//!   segmentation must be byte-identical;
//! * a server without `Range` support → single connection;
//! * pause / resume keeps progress; chunked bodies of unknown size; retries
//!   after server errors; Content-Disposition names and unique file names.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zenless_dm::engine::{Command, Engine, EngineConfig, Id, NewDownload, StartMode, Status, UiEvent};
use zenless_dm::settings::Settings;

// ---------------------------------------------------------------------------
// Test server
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Ranges,
    NoRanges,
    Chunked,
}

struct ServerOpts {
    mode: Mode,
    /// Sleep per 64 KiB chunk for requests starting at byte 0 (makes the
    /// first connection slow so others finish early and split its segment).
    slow_first: Option<Duration>,
    /// Sleep per 64 KiB chunk for every request.
    slow_all: Option<Duration>,
    /// Reply 503 to this many requests first.
    fail_first: usize,
    content_disposition: Option<&'static str>,
}

impl Default for ServerOpts {
    fn default() -> Self {
        Self { mode: Mode::Ranges, slow_first: None, slow_all: None, fail_first: 0, content_disposition: None }
    }
}

struct TestServer {
    base: String,
    /// `(start, end)` of every range request served.
    ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    requests: Arc<AtomicUsize>,
    /// Most partial (segment) transfers that were in flight at the same time.
    peak_parallel: Arc<AtomicUsize>,
}

/// Counts partial-content transfers in flight (the probe's full range excluded).
#[derive(Default)]
struct Parallel {
    now: AtomicUsize,
    peak: Arc<AtomicUsize>,
}

struct SlowReader {
    data: Arc<Vec<u8>>,
    pos: usize,
    end: usize,
    delay: Option<Duration>,
}

impl Read for SlowReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.end {
            return Ok(0);
        }
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        let n = buf.len().min(64 * 1024).min(self.end - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn parse_range(v: &str) -> Option<(u64, Option<u64>)> {
    let r = v.trim().strip_prefix("bytes=")?;
    let (a, b) = r.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()))
}

fn payload(len: usize) -> Arc<Vec<u8>> {
    // Deterministic, non-repeating-ish bytes (xorshift).
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    Arc::new(
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect(),
    )
}

fn start_server(data: Arc<Vec<u8>>, opts: ServerOpts) -> TestServer {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind test server");
    let port = server.server_addr().to_ip().expect("ip addr").port();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::new(AtomicUsize::new(0));
    let parallel = Arc::new(Parallel::default());
    let peak_parallel = parallel.peak.clone();
    let opts = Arc::new(opts);
    {
        let ranges = ranges.clone();
        let requests = requests.clone();
        std::thread::spawn(move || {
            for rq in server.incoming_requests() {
                let data = data.clone();
                let ranges = ranges.clone();
                let opts = opts.clone();
                let parallel = parallel.clone();
                let n = requests.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || serve(rq, data, &opts, &ranges, &parallel, n));
            }
        });
    }
    TestServer { base: format!("http://127.0.0.1:{port}"), ranges, requests, peak_parallel }
}

fn serve(
    rq: tiny_http::Request,
    data: Arc<Vec<u8>>,
    opts: &ServerOpts,
    ranges: &Mutex<Vec<(u64, u64)>>,
    parallel: &Parallel,
    n: usize,
) {
    use tiny_http::{Header, Response, StatusCode};
    if n < opts.fail_first {
        let _ = rq.respond(Response::from_string("busy").with_status_code(503));
        return;
    }
    let total = data.len() as u64;
    let range = rq
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| parse_range(h.value.as_str()));
    let mut headers = vec![Header::from_bytes("Content-Type", "application/octet-stream").unwrap()];
    if let Some(cd) = opts.content_disposition {
        headers.push(Header::from_bytes("Content-Disposition", cd).unwrap());
    }
    let (status, start, end) = match (opts.mode, range) {
        (Mode::Ranges, Some((s, e))) if s < total => {
            let end = e.map_or(total - 1, |e| e.min(total - 1));
            ranges.lock().unwrap().push((s, end + 1));
            headers.push(Header::from_bytes("Content-Range", format!("bytes {s}-{end}/{total}")).unwrap());
            headers.push(Header::from_bytes("Accept-Ranges", "bytes").unwrap());
            (206, s, end + 1)
        }
        _ => (200, 0, total),
    };
    let delay = if start == 0 { opts.slow_first.or(opts.slow_all) } else { opts.slow_all };
    let reader = SlowReader { data, pos: start as usize, end: end as usize, delay };
    let len = if opts.mode == Mode::Chunked { None } else { Some((end - start) as usize) };
    let resp = Response::new(StatusCode(status), headers, reader, len, None);
    let segment = status == 206 && end - start < total;
    if segment {
        let now = parallel.now.fetch_add(1, Ordering::SeqCst) + 1;
        parallel.peak.fetch_max(now, Ordering::SeqCst);
    }
    let _ = rq.respond(resp);
    if segment {
        parallel.now.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Engine helpers
// ---------------------------------------------------------------------------

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zdm-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn engine(dir: &Path) -> (Engine, Receiver<UiEvent>) {
    engine_with(dir, |_| {})
}

fn engine_with(dir: &Path, tweak: impl FnOnce(&mut Settings)) -> (Engine, Receiver<UiEvent>) {
    let (tx, rx) = channel();
    let mut settings = Settings {
        download_dir: dir.to_path_buf(),
        max_retries: 4,
        timeout_secs: 10,
        ..Settings::default()
    };
    tweak(&mut settings);
    let cfg = EngineConfig { settings, downloads: Vec::new(), data_dir: None, demo: false };
    (Engine::start(cfg, tx, Arc::new(|| {})).expect("engine"), rx)
}

fn add(engine: &Engine, url: &str, connections: u32) -> Id {
    let mut n = NewDownload::new(url, StartMode::Now);
    n.connections = Some(connections);
    engine.add(n)
}

/// Waits for the download to finish and returns the final path.
fn wait_finished(rx: &Receiver<UiEvent>, id: Id, timeout: Duration) -> PathBuf {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(UiEvent::Finished { id: i, path, .. }) if i == id => return path,
            Ok(UiEvent::Failed { id: i, error, .. }) if i == id => panic!("download failed: {error}"),
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for download {id}"),
        }
    }
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < deadline, "condition not reached in time");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn leftover_temp_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".zdm"))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn multi_connection_download_is_byte_identical() {
    let data = payload(24 * 1024 * 1024 + 12_345);
    let srv = start_server(
        data.clone(),
        ServerOpts { slow_first: Some(Duration::from_millis(15)), ..Default::default() },
    );
    let dir = temp_dir("multi");
    let (engine, rx) = engine(&dir);
    let id = add(&engine, &format!("{}/files/big%20file.bin", srv.base), 4);
    let path = wait_finished(&rx, id, Duration::from_secs(120));
    assert_eq!(path.file_name().unwrap(), "big file.bin");
    let got = std::fs::read(&path).unwrap();
    assert_eq!(got.len(), data.len());
    assert!(got == *data, "content differs");
    assert!(leftover_temp_files(&dir).is_empty());
    // Dynamic segmentation: the slow first segment must have been split, so
    // more range requests than connections were made (plus the probe).
    let ranges = srv.ranges.lock().unwrap().clone();
    let transfer_ranges: Vec<_> = ranges.iter().filter(|r| r.1 - r.0 < data.len() as u64).collect();
    assert!(transfer_ranges.len() > 4, "expected dynamic splits, got {ranges:?}");
    let snap = engine.snapshot();
    let d = snap.get(id).unwrap();
    assert_eq!(d.status, Status::Completed);
    assert_eq!(d.resumable, Some(true));
    assert_eq!(d.downloaded, data.len() as u64);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Downloads a 6 MiB file (each 64 KiB chunk slowed down so transfers
/// overlap) and returns (peak parallel segment transfers, connections recorded).
fn small_file_run(name: &str, connections: Option<u32>, tweak: impl FnOnce(&mut Settings)) -> (usize, u32) {
    let data = payload(6 * 1024 * 1024 + 5);
    let srv = start_server(data.clone(), ServerOpts { slow_all: Some(Duration::from_millis(4)), ..Default::default() });
    let dir = temp_dir(name);
    let (engine, rx) = engine_with(&dir, tweak);
    let mut n = NewDownload::new(format!("{}/files/{name}.bin", srv.base), StartMode::Now);
    n.connections = connections;
    let id = engine.add(n);
    let path = wait_finished(&rx, id, Duration::from_secs(120));
    assert!(std::fs::read(&path).unwrap() == *data, "content differs");
    let connections = engine.snapshot().get(id).unwrap().connections;
    let _ = std::fs::remove_dir_all(&dir);
    (srv.peak_parallel.load(Ordering::SeqCst), connections)
}

#[test]
fn small_files_use_two_connections_by_default() {
    // Default settings: ≤ 100 MB and no hand-picked count → at most 2 connections.
    let (peak, connections) = small_file_run("small-auto", None, |_| {});
    assert!(peak <= 2, "expected at most 2 parallel transfers, saw {peak}");
    assert_eq!(connections, 2, "the download should record the 2 connections it used");

    // A hand-picked count wins over the rule.
    let (peak, connections) = small_file_run("small-picked", Some(8), |_| {});
    assert!(peak > 2, "8 hand-picked connections should run in parallel, saw {peak}");
    assert_eq!(connections, 8);

    // Bigger than the threshold (lowered to 1 MB here) → the default 8.
    let (peak, connections) = small_file_run("over-threshold", None, |s| s.small_file_mb = 1);
    assert!(peak > 2, "a file over the threshold should use the default count, saw {peak}");
    assert_eq!(connections, 8);
}

#[test]
fn server_without_range_support_uses_one_connection() {
    let data = payload(3 * 1024 * 1024 + 7);
    let srv = start_server(data.clone(), ServerOpts { mode: Mode::NoRanges, ..Default::default() });
    let dir = temp_dir("norange");
    let (engine, rx) = engine(&dir);
    let id = add(&engine, &format!("{}/download/archive.zip", srv.base), 8);
    let path = wait_finished(&rx, id, Duration::from_secs(60));
    assert_eq!(std::fs::read(&path).unwrap(), *data);
    let snap = engine.snapshot();
    let d = snap.get(id).unwrap();
    assert_eq!(d.resumable, Some(false));
    assert!(d.note.is_some(), "user should be told that resume isn't supported");
    // Probe + exactly one transfer request.
    assert_eq!(srv.requests.load(Ordering::SeqCst), 2);
    assert!(leftover_temp_files(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn chunked_body_of_unknown_size() {
    let data = payload(1024 * 1024 + 99);
    let srv = start_server(data.clone(), ServerOpts { mode: Mode::Chunked, ..Default::default() });
    let dir = temp_dir("chunked");
    let (engine, rx) = engine(&dir);
    let id = add(&engine, &format!("{}/stream", srv.base), 8);
    let path = wait_finished(&rx, id, Duration::from_secs(60));
    assert_eq!(std::fs::read(&path).unwrap(), *data);
    // "stream" + application/octet-stream → no extension to add.
    assert_eq!(path.file_name().unwrap(), "stream");
    let snap = engine.snapshot();
    assert_eq!(snap.get(id).unwrap().total_size, Some(data.len() as u64));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pause_and_resume_keep_progress() {
    let data = payload(8 * 1024 * 1024 + 1);
    let srv = start_server(
        data.clone(),
        ServerOpts { slow_all: Some(Duration::from_millis(40)), ..Default::default() },
    );
    let dir = temp_dir("pause");
    let (engine, rx) = engine(&dir);
    let id = add(&engine, &format!("{}/pausable.bin", srv.base), 4);
    wait_until(Duration::from_secs(30), || {
        engine.snapshot().get(id).is_some_and(|d| d.downloaded > 512 * 1024)
    });
    engine.send(Command::Pause(vec![id]));
    wait_until(Duration::from_secs(10), || {
        engine.snapshot().get(id).is_some_and(|d| d.status == Status::Paused && d.rt.conns.is_empty())
    });
    let before = engine.snapshot().get(id).unwrap().downloaded;
    assert!(before > 0);
    assert_eq!(leftover_temp_files(&dir), vec!["pausable.bin.zdm".to_owned()]);
    let served_before = srv.ranges.lock().unwrap().len();
    engine.send(Command::Resume(vec![id]));
    let path = wait_finished(&rx, id, Duration::from_secs(120));
    assert_eq!(std::fs::read(&path).unwrap(), *data);
    // After resuming, transfers continued from where they were, not from 0.
    let ranges = srv.ranges.lock().unwrap().clone();
    let resumed: Vec<_> = ranges[served_before..].iter().filter(|r| r.0 != 0 || r.1 != data.len() as u64).collect();
    assert!(!resumed.is_empty());
    assert!(resumed.iter().all(|r| r.0 != 0), "resume must not restart from byte 0: {resumed:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retries_after_server_errors() {
    let data = payload(2 * 1024 * 1024);
    let srv = start_server(data.clone(), ServerOpts { fail_first: 2, ..Default::default() });
    let dir = temp_dir("retry");
    let (engine, rx) = engine(&dir);
    let id = add(&engine, &format!("{}/flaky.iso", srv.base), 2);
    let path = wait_finished(&rx, id, Duration::from_secs(60));
    assert_eq!(std::fs::read(&path).unwrap(), *data);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn content_disposition_and_unique_names() {
    let data = payload(300 * 1024);
    let srv = start_server(
        data.clone(),
        ServerOpts {
            content_disposition: Some("attachment; filename=\"fallback.txt\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf"),
            ..Default::default()
        },
    );
    let dir = temp_dir("names");
    let (engine, rx) = engine(&dir);
    let url = format!("{}/get?id=1", srv.base);
    let a = add(&engine, &url, 4);
    let pa = wait_finished(&rx, a, Duration::from_secs(60));
    let b = add(&engine, &url, 4);
    let pb = wait_finished(&rx, b, Duration::from_secs(60));
    assert_eq!(pa.file_name().unwrap(), "résumé.pdf");
    assert_eq!(pb.file_name().unwrap(), "résumé (1).pdf");
    assert_eq!(std::fs::read(&pb).unwrap(), *data);
    let snap = engine.snapshot();
    assert_eq!(snap.get(a).unwrap().category, zenless_dm::category::Category::Documents);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn queue_respects_max_concurrent() {
    let data = payload(2 * 1024 * 1024);
    let srv = start_server(
        data.clone(),
        ServerOpts { slow_all: Some(Duration::from_millis(30)), ..Default::default() },
    );
    let dir = temp_dir("queue");
    let (tx, rx) = channel();
    let settings = Settings { download_dir: dir.clone(), max_concurrent: 1, ..Settings::default() };
    let engine = Engine::start(
        EngineConfig { settings, downloads: Vec::new(), data_dir: None, demo: false },
        tx,
        Arc::new(|| {}),
    )
    .unwrap();
    let ids: Vec<Id> = (0..3)
        .map(|i| {
            let mut n = NewDownload::new(format!("{}/q{i}.bin", srv.base), StartMode::Queue);
            n.connections = Some(2);
            engine.add(n)
        })
        .collect();
    // Only one may be active at any time.
    let mut finished = 0;
    let deadline = Instant::now() + Duration::from_secs(120);
    while finished < 3 {
        let snap = engine.snapshot();
        assert!(snap.count(|d| d.status.is_active()) <= 1);
        while let Ok(ev) = rx.try_recv() {
            if let UiEvent::Finished { .. } = ev {
                finished += 1;
            }
            if let UiEvent::Failed { error, .. } = ev {
                panic!("failed: {error}");
            }
        }
        assert!(Instant::now() < deadline, "queue did not finish");
        std::thread::sleep(Duration::from_millis(25));
    }
    for id in ids {
        let d = engine.snapshot().get(id).cloned().unwrap();
        assert_eq!(std::fs::read(d.path()).unwrap(), *data);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
