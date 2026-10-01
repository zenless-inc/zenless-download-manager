//! Self-updater for a Zenless app that ships as a single `.exe` attached to
//! its GitHub releases.
//!
//! How it works:
//! * A background thread asks `GET {api}/repos/{repo}/releases/latest`
//!   (`api` = `https://api.github.com`, or `ZENLESS_UPDATE_API` in tests):
//!   about 15 s after start when the last check is older than 6 h, then every
//!   6 h while the app runs. "Check for updates" always checks.
//! * A newer release is downloaded to `<data_dir>\updates\<app_id>-<version>.part`
//!   and only accepted when its size, SHA-256 (the asset's `digest`, else a
//!   sibling `<asset>.sha256` file) and `MZ` header match. Releases without a
//!   checksum are refused. The verified file is renamed to `.exe` → [`Status::Ready`].
//! * Installing renames the running exe to `<exe>.old` (Windows allows
//!   renaming a running program), copies the verified file into its place and
//!   either starts it right away with `--updated-from <old version>`
//!   ([`Updater::install_and_restart`]) or leaves it for the next start
//!   ([`Updater::install_on_exit`]). The next start deletes `<exe>.old`.
//! * The UI reads [`Status`] every frame: [`Updater::banner_ui`] for the main
//!   window, [`Updater::settings_ui`] for Settings › About. Nothing blocks the
//!   UI thread; the worker calls `ctx.request_repaint()` when the status changes.
//! * Apps can piggyback their own work on every check with [`Updater::on_check`]
//!   (the Download Manager refreshes the browser extensions that way).
//!
//! Testing aids: `ZENLESS_UPDATE_API=http://127.0.0.1:<port>` (a mock of the
//! GitHub API) and `ZENLESS_UPDATE_DELAY_SECS=<n>` (first automatic check
//! after n seconds, even when the last one was recent). What the updater did
//! is logged to `<data_dir>\updates\updater.log`.
//!
//! Needs `reqwest` (with the `blocking` feature), `sha2`, `serde`/`serde_json`,
//! `eframe` and `egui-phosphor`; the tests use `tiny_http`.
//!
//! This file is shared verbatim between Zenless Download Manager and Zenless
//! Torrent. Keep the copies in sync; app-specific code belongs in the apps.

use super::kit;
use super::theme::{Palette, alpha, mix};
use eframe::egui::{self, Color32, RichText, Stroke, vec2};
use egui_phosphor::regular as ph;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

/// The real GitHub API.
pub const GITHUB_API: &str = "https://api.github.com";
/// Why a release without any checksum is refused.
pub const NO_CHECKSUM: &str = "This release has no checksum, so it can't be verified";
/// First bytes of every Windows program.
pub const EXE_MAGIC: &[u8] = b"MZ";

const PREFS_FILE: &str = "updater.json";
const LOG_FILE: &str = "updater.log";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(15);
const API_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest silence tolerated while a download streams.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Configuration, preferences, status
// ---------------------------------------------------------------------------

/// What the updater needs to know about the app.
#[derive(Clone, Debug)]
pub struct UpdaterConfig {
    /// `"zenless-dm"` | `"zenless-torrent"` (User-Agent, file names).
    pub app_id: &'static str,
    /// `"Zenless Download Manager"`.
    pub app_name: &'static str,
    /// `"zenless-inc/zenless-download-manager"`.
    pub repo: &'static str,
    /// The release asset to install, e.g. `"zenless-dm.exe"`.
    pub asset: &'static str,
    /// `env!("CARGO_PKG_VERSION")`.
    pub current_version: &'static str,
    /// `%APPDATA%\Zenless\<App>`: `updater.json` and the `updates\` folder.
    pub data_dir: PathBuf,
}

impl UpdaterConfig {
    /// `User-Agent` of every request, e.g. `zenless-dm/0.1.0`.
    pub fn user_agent(&self) -> String {
        format!("{}/{}", self.app_id, self.current_version)
    }

    /// Where downloads wait until they are installed.
    pub fn updates_dir(&self) -> PathBuf {
        self.data_dir.join("updates")
    }

    /// The GitHub page of a release ("What's new").
    pub fn release_page(&self, version: &str) -> String {
        format!("https://github.com/{}/releases/tag/v{}", self.repo, version_from_tag(version))
    }
}

/// `<data_dir>\updater.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePrefs {
    /// Check ~15 s after start and every 6 hours.
    pub auto_check: bool,
    /// Download in the background and install when the app closes.
    pub auto_install: bool,
    /// "Skip this version".
    pub skipped_version: Option<String>,
    /// Unix seconds of the last finished check (0 = never).
    pub last_check: u64,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self { auto_check: true, auto_install: true, skipped_version: None, last_check: 0 }
    }
}

impl UpdatePrefs {
    pub fn load(data_dir: &Path) -> Self {
        fs::read(data_dir.join(PREFS_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(&data_dir.join(PREFS_FILE), &json)
    }
}

/// A published release, as far as the updater cares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// `0.2.0` (the tag without its leading `v`).
    pub version: String,
    /// `v0.2.0`.
    pub tag: String,
    /// Release notes (GitHub-flavoured markdown).
    pub notes: String,
    /// The release page on GitHub.
    pub html_url: String,
    /// Download URL of the asset (empty if the release lacks it).
    pub asset_url: String,
    pub asset_size: u64,
    /// Lower-case hex SHA-256 of the asset, if the release provides one.
    pub sha256: Option<String>,
    /// ISO 8601, e.g. `2026-10-01T12:00:00Z`.
    pub published_at: String,
}

impl ReleaseInfo {
    pub fn has_asset(&self) -> bool {
        !self.asset_url.is_empty()
    }
}

/// What the updater is doing. The UI reads it every frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate { checked_at: u64 },
    Available(ReleaseInfo),
    Downloading { release: ReleaseInfo, received: u64, total: u64 },
    Ready { release: ReleaseInfo, path: PathBuf },
    Installing,
    Failed { message: String, release: Option<ReleaseInfo> },
}

impl Status {
    /// The release this status is about, if any.
    pub fn release(&self) -> Option<&ReleaseInfo> {
        match self {
            Status::Available(r)
            | Status::Downloading { release: r, .. }
            | Status::Ready { release: r, .. }
            | Status::Failed { release: Some(r), .. } => Some(r),
            _ => None,
        }
    }

    fn busy(&self) -> bool {
        matches!(self, Status::Checking | Status::Downloading { .. } | Status::Installing)
    }
}

/// Something the app has to do in response to a click in the updater UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    /// "Restart now": the app collects what it wants to pass on (e.g. which
    /// transfers to resume), calls [`Updater::install_and_restart`] and, on
    /// success, saves its state and exits.
    RestartNow,
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

/// `major.minor.patch[-pre]` (a missing minor/patch counts as 0, build
/// metadata after `+` is ignored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Pre-release identifiers (`beta.2`); `None` for a plain release.
    pub pre: Option<String>,
}

fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl Version {
    /// Accepts `1.2.3`, `v1.2.3`, `1.2`, `1.2.3-beta.1`, `1.2.3+build.5`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = version_from_tag(s);
        let s = s.split('+').next().unwrap_or_default();
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        if pre.is_some_and(|p| p.is_empty() || p.split('.').any(str::is_empty)) {
            return None;
        }
        let mut parts = core.split('.');
        let major = number(parts.next()?)?;
        let minor = match parts.next() {
            Some(m) => number(m)?,
            None => 0,
        };
        let patch = match parts.next() {
            Some(p) => number(p)?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Self { major, minor, patch, pre: pre.map(str::to_owned) })
    }
}

/// SemVer precedence of pre-release identifiers: numeric ones compare
/// numerically and sort before alphanumeric ones; more identifiers win a tie.
fn cmp_pre(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.split('.'), b.split('.'));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let o = match (number(p), number(q)) {
                    (Some(m), Some(n)) => m.cmp(&n),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => p.cmp(q),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater, // 1.0.0 > 1.0.0-beta
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => cmp_pre(a, b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Is `candidate` a newer version than `current`? Unparsable → `false`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// `v1.2.3` → `1.2.3`.
pub fn version_from_tag(tag: &str) -> &str {
    let t = tag.trim();
    t.strip_prefix(['v', 'V']).unwrap_or(t)
}

// ---------------------------------------------------------------------------
// Checksums
// ---------------------------------------------------------------------------

fn normalize_sha256(hex: &str) -> Option<String> {
    let h = hex.trim().to_ascii_lowercase();
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some(h)
}

/// GitHub's asset `digest` (`"sha256:<hex>"`) → lower-case hex.
pub fn parse_digest(digest: &str) -> Option<String> {
    let (algo, hex) = digest.trim().split_once(':')?;
    if !algo.eq_ignore_ascii_case("sha256") {
        return None;
    }
    normalize_sha256(hex)
}

/// A `sha256sum`-style file (`<hex>  <name>`) → lower-case hex.
pub fn parse_sha256_file(text: &str) -> Option<String> {
    normalize_sha256(text.split_whitespace().next()?)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 and the first 16 bytes of a file.
fn hash_file(path: &Path) -> io::Result<(String, Vec<u8>)> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut head = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if head.len() < 16 {
            let take = (16 - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        hasher.update(&buf[..n]);
    }
    Ok((to_hex(&hasher.finalize()), head))
}

/// Lower-case hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    hash_file(path).map(|(h, _)| h)
}

/// Checks a file against the size (0 = don't care), checksum and header a
/// release promises.
pub fn verify_file(path: &Path, size: u64, sha256: Option<&str>, magic: &[u8]) -> Result<(), String> {
    let expected = sha256.and_then(normalize_sha256).ok_or_else(|| NO_CHECKSUM.to_owned())?;
    let len = fs::metadata(path).map(|m| m.len()).map_err(|e| format!("{}: {e}", path.display()))?;
    if size > 0 && len != size {
        return Err(format!("The downloaded file has the wrong size ({len} instead of {size} bytes)"));
    }
    let (hash, head) = hash_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if hash != expected {
        return Err("The download doesn't match its SHA-256 checksum, so it was discarded".to_owned());
    }
    if !head.starts_with(magic) {
        return Err("The downloaded file isn't in the expected format, so it was discarded".to_owned());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GitHub API
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// A release from the GitHub API, plus the URL of its `<asset>.sha256`
/// file (used when the asset itself has no `digest`).
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedRelease {
    pub info: ReleaseInfo,
    pub checksum_url: Option<String>,
}

/// Parses a `releases/latest` answer and picks the asset named exactly `asset`.
pub fn parse_release(json: &str, asset: &str) -> Result<ParsedRelease, String> {
    let r: GhRelease =
        serde_json::from_str(json).map_err(|e| format!("The update server sent an unexpected answer ({e})"))?;
    let sha_name = format!("{asset}.sha256");
    let checksum_url = r
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(&sha_name))
        .map(|a| a.browser_download_url.clone());
    let found = r.assets.iter().find(|a| a.name == asset);
    let info = ReleaseInfo {
        version: version_from_tag(&r.tag_name).to_owned(),
        tag: r.tag_name.clone(),
        notes: r.body.unwrap_or_default(),
        html_url: r.html_url.unwrap_or_default(),
        asset_url: found.map(|a| a.browser_download_url.clone()).unwrap_or_default(),
        asset_size: found.map_or(0, |a| a.size),
        sha256: found.and_then(|a| a.digest.as_deref()).and_then(parse_digest),
        published_at: r.published_at.unwrap_or_default(),
    };
    Ok(ParsedRelease { info, checksum_url })
}

/// `ZENLESS_UPDATE_API` (tests) or the real GitHub API.
pub fn api_base() -> String {
    std::env::var("ZENLESS_UPDATE_API")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| GITHUB_API.to_owned())
}

fn http_client(user_agent: &str, timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(API_TIMEOUT)
        .timeout(timeout)
        .build()
        .map_err(|e| format!("Couldn't set up networking ({e})"))
}

/// A short description of a network error (its innermost cause).
fn describe(e: &reqwest::Error) -> String {
    let mut msg = if e.is_timeout() {
        "the connection timed out".to_owned()
    } else if e.is_connect() {
        "couldn't connect".to_owned()
    } else {
        "network error".to_owned()
    };
    let mut cause = std::error::Error::source(e);
    let mut last = None;
    while let Some(c) = cause {
        last = Some(c.to_string());
        cause = c.source();
    }
    if let Some(l) = last {
        msg.push_str(&format!(" ({l})"));
    }
    msg
}

/// `GET {api}/repos/{repo}/releases/latest`, with the `.sha256` fallback.
pub fn fetch_latest(api: &str, repo: &str, asset: &str, user_agent: &str) -> Result<ReleaseInfo, String> {
    let http = http_client(user_agent, API_TIMEOUT)?;
    let url = format!("{}/repos/{repo}/releases/latest", api.trim_end_matches('/'));
    let resp = http
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Couldn't reach the update server: {}", describe(&e)))?;
    match resp.status().as_u16() {
        200..=299 => {}
        404 => return Err("No release has been published yet".to_owned()),
        403 | 429 => return Err("GitHub's rate limit was reached. Try again later.".to_owned()),
        s => return Err(format!("The update server answered HTTP {s}")),
    }
    let text = resp.text().map_err(|e| format!("Couldn't read the update information: {}", describe(&e)))?;
    let parsed = parse_release(&text, asset)?;
    let mut info = parsed.info;
    if info.sha256.is_none()
        && let Some(sum_url) = parsed.checksum_url
    {
        info.sha256 = fetch_checksum_file(&http, &sum_url);
    }
    Ok(info)
}

fn fetch_checksum_file(http: &reqwest::blocking::Client, url: &str) -> Option<String> {
    let resp = http.get(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let mut text = String::new();
    resp.take(4096).read_to_string(&mut text).ok()?;
    parse_sha256_file(&text)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Downloads `url` to `dest` (through `<dest>.part`) and verifies size
/// (`size` 0 = unknown), SHA-256 and the leading `magic` bytes. `progress`
/// gets `(received, total)`; returning `false` cancels. A `dest` that is
/// already there and verifies is reused. Nothing is left behind on failure.
pub fn download_verified(
    url: &str,
    user_agent: &str,
    size: u64,
    sha256: Option<&str>,
    magic: &[u8],
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<(), String> {
    let expected = sha256.and_then(normalize_sha256).ok_or_else(|| NO_CHECKSUM.to_owned())?;
    if url.is_empty() {
        return Err("This release has nothing to download for this app".to_owned());
    }
    if dest.is_file() {
        if verify_file(dest, size, Some(&expected), magic).is_ok() {
            progress(size, size);
            return Ok(());
        }
        let _ = fs::remove_file(dest);
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    }
    let part = with_suffix(dest, ".part");
    let result = fetch_to(url, user_agent, size, &expected, magic, &part, progress)
        .and_then(|()| fs::rename(&part, dest).map_err(|e| format!("Couldn't save the download ({e})")));
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

fn fetch_to(
    url: &str,
    user_agent: &str,
    size: u64,
    expected: &str,
    magic: &[u8],
    part: &Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<(), String> {
    let http = http_client(user_agent, STALL_TIMEOUT)?;
    let mut resp = http.get(url).send().map_err(|e| format!("The download failed: {}", describe(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("The download failed (HTTP {})", resp.status().as_u16()));
    }
    let total = if size > 0 { size } else { resp.content_length().unwrap_or(0) };
    let mut file = fs::File::create(part).map_err(|e| format!("Couldn't save the download ({e})"))?;
    let mut hasher = Sha256::new();
    let mut head: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut received = 0u64;
    if !progress(0, total) {
        return Err("Cancelled".to_owned());
    }
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("The download was interrupted ({e})"))?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if size > 0 && received > size {
            return Err("The download is larger than the release says, so it was discarded".to_owned());
        }
        if head.len() < magic.len() {
            let take = (magic.len() - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| format!("Couldn't save the download ({e})"))?;
        if !progress(received, total) {
            return Err("Cancelled".to_owned());
        }
    }
    file.sync_all().map_err(|e| format!("Couldn't save the download ({e})"))?;
    drop(file);
    if size > 0 && received != size {
        return Err(format!(
            "The download is incomplete ({} of {})",
            kit::human_bytes(received),
            kit::human_bytes(size)
        ));
    }
    if to_hex(&hasher.finalize()) != expected {
        return Err("The download doesn't match its SHA-256 checksum, so it was discarded".to_owned());
    }
    if !head.starts_with(magic) {
        return Err("The downloaded file isn't in the expected format, so it was discarded".to_owned());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Installing
// ---------------------------------------------------------------------------

/// `<exe>.old`: where the previous version waits to be deleted.
pub fn old_exe_path(exe: &Path) -> PathBuf {
    with_suffix(exe, ".old")
}

fn cant_replace(exe: &Path, e: &io::Error) -> String {
    let dir = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let reason = if e.kind() == io::ErrorKind::PermissionDenied { "access denied".to_owned() } else { e.to_string() };
    format!("Zenless can't replace itself in {dir} ({reason}). Download the new version from the website instead.")
}

/// Puts `new_file` where `exe` is and keeps the previous program as
/// `<exe>.old`. Works while `exe` runs; rolls back on any error.
pub fn replace_exe(exe: &Path, new_file: &Path) -> Result<(), String> {
    let old = old_exe_path(exe);
    if old.exists() {
        fs::remove_file(&old).map_err(|e| cant_replace(exe, &e))?;
    }
    fs::rename(exe, &old).map_err(|e| cant_replace(exe, &e))?;
    if let Err(e) = fs::copy(new_file, exe) {
        restore_exe(exe);
        return Err(cant_replace(exe, &e));
    }
    Ok(())
}

/// Undoes [`replace_exe`].
fn restore_exe(exe: &Path) {
    let old = old_exe_path(exe);
    if old.exists() {
        // `rename` replaces a (partially) copied new file.
        let _ = fs::rename(&old, exe);
    }
}

/// Called first thing by a freshly updated app (`--updated-from`), before its
/// single-instance check: waits until the previous instance stops answering
/// `GET http://127.0.0.1:{port}/ping` (at most 20 s).
pub fn wait_for_previous_instance(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && local_ping(port, Duration::from_millis(300)) {
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn local_ping(port: u16, timeout: Duration) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let req = format!("GET /ping HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut head = [0u8; 16];
    matches!(stream.read(&mut head), Ok(n) if n >= 5 && head.starts_with(b"HTTP/"))
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = with_suffix(path, ".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// Appends a line to `<data_dir>\updates\updater.log` (kept below ~256 KB).
pub fn log_line(data_dir: &Path, msg: &str) {
    let dir = data_dir.join("updates");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(LOG_FILE);
    if fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = fs::rename(&path, dir.join(format!("{LOG_FILE}.1")));
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = kit::unix_now();
        let _ = writeln!(f, "{}:{:02} UTC  {msg}", kit::human_date(now), now % 60);
    }
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago".
fn ago(unix: u64) -> String {
    let s = kit::unix_now().saturating_sub(unix);
    match s {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        86_400..=172_799 => "yesterday".to_owned(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The first lines of the release notes as plain text.
fn notes_preview(markdown: &str, max_lines: usize) -> String {
    let mut lines = markdown
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("<!--"))
        .map(|l| {
            let l = l.trim_start_matches('#').trim();
            let l = match l.strip_prefix("* ").or_else(|| l.strip_prefix("- ")) {
                Some(rest) => format!("•  {rest}"),
                None => l.to_owned(),
            };
            l.replace("**", "").replace('`', "")
        });
    let mut out: Vec<String> = lines.by_ref().take(max_lines).collect();
    if lines.next().is_some()
        && let Some(last) = out.last_mut()
    {
        last.push_str(" …");
    }
    out.join("\n")
}

// ---------------------------------------------------------------------------
// The updater
// ---------------------------------------------------------------------------

/// Handed to [`Updater::on_check`] hooks on the worker thread.
pub struct CheckContext<'a> {
    pub config: &'a UpdaterConfig,
    /// API base URL (GitHub or the test mock).
    pub api: &'a str,
    /// "Check for updates" was clicked (otherwise a scheduled check).
    pub manual: bool,
    ctx: Option<&'a egui::Context>,
    cancel: &'a AtomicBool,
}

impl CheckContext<'_> {
    /// The app is installing an update or closing: stop early.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(AtomicOrdering::SeqCst)
    }

    pub fn log(&self, msg: &str) {
        log_line(&self.config.data_dir, msg);
    }

    pub fn request_repaint(&self) {
        if let Some(ctx) = self.ctx {
            ctx.request_repaint();
        }
    }
}

/// Extra work done after every check (see [`Updater::on_check`]).
pub type CheckHook = Box<dyn FnMut(&CheckContext<'_>) + Send>;

enum Cmd {
    Check { manual: bool },
    Download,
    Reschedule,
}

struct Shared {
    status: Mutex<Status>,
    prefs: Mutex<UpdatePrefs>,
    ctx: Mutex<Option<egui::Context>>,
    /// Set while installing / exiting: background work stops early.
    cancel: AtomicBool,
    /// Held by the worker while it checks or downloads.
    work: Mutex<()>,
    /// The user clicked "Update now": install on exit even without auto-install.
    requested: AtomicBool,
}

impl Shared {
    fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    fn set(&self, status: Status) {
        *lock(&self.status) = status;
        if let Some(ctx) = lock(&self.ctx).as_ref() {
            ctx.request_repaint();
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(AtomicOrdering::SeqCst)
    }
}

/// The app-side handle: owns the worker thread and draws the updater UI.
pub struct Updater {
    cfg: Arc<UpdaterConfig>,
    shared: Arc<Shared>,
    tx: Option<Sender<Cmd>>,
    hook: Option<CheckHook>,
    api: Option<String>,
    /// "Later": the version whose banner is hidden for this session.
    hidden: Option<String>,
}

impl Updater {
    /// Loads the preferences. Nothing runs until [`Updater::start`].
    pub fn new(cfg: UpdaterConfig) -> Self {
        let prefs = UpdatePrefs::load(&cfg.data_dir);
        Self {
            cfg: Arc::new(cfg),
            shared: Arc::new(Shared {
                status: Mutex::new(Status::Idle),
                prefs: Mutex::new(prefs),
                ctx: Mutex::new(None),
                cancel: AtomicBool::new(false),
                work: Mutex::new(()),
                requested: AtomicBool::new(false),
            }),
            tx: None,
            hook: None,
            api: None,
            hidden: None,
        }
    }

    /// Uses another API base URL instead of `ZENLESS_UPDATE_API` / GitHub (tests).
    pub fn with_api(mut self, api: impl Into<String>) -> Self {
        self.api = Some(api.into());
        self
    }

    /// Runs `hook` on the worker thread after every check (manual or
    /// scheduled). Call before [`Updater::start`].
    pub fn on_check(&mut self, hook: impl FnMut(&CheckContext<'_>) + Send + 'static) {
        self.hook = Some(Box::new(hook));
    }

    /// Starts the background worker (cleans up leftovers of the previous
    /// update, then checks on schedule).
    pub fn start(&mut self, ctx: &egui::Context) {
        if self.tx.is_some() {
            return;
        }
        *lock(&self.shared.ctx) = Some(ctx.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = self.cfg.clone();
        let shared = self.shared.clone();
        let hook = self.hook.take();
        let api = self.api.clone().unwrap_or_else(api_base);
        let spawned = std::thread::Builder::new()
            .name(format!("{}-updater", cfg.app_id))
            .spawn(move || worker(cfg, shared, api, rx, hook));
        if spawned.is_ok() {
            self.tx = Some(tx);
        }
    }

    pub fn config(&self) -> &UpdaterConfig {
        &self.cfg
    }

    pub fn status(&self) -> Status {
        self.shared.status()
    }

    pub fn prefs(&self) -> UpdatePrefs {
        lock(&self.shared.prefs).clone()
    }

    fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    /// "Check for updates".
    pub fn check_now(&self) {
        if !self.status().busy() {
            self.send(Cmd::Check { manual: true });
        }
    }

    /// "Update now": downloads the available release (it installs on exit,
    /// or with "Restart now").
    pub fn download(&self) {
        self.shared.requested.store(true, AtomicOrdering::SeqCst);
        self.send(Cmd::Download);
    }

    /// "Skip this version".
    pub fn skip_version(&self) {
        let Some(version) = self.status().release().map(|r| r.version.clone()) else { return };
        self.update_prefs(|p| p.skipped_version = Some(version));
        self.shared.set(Status::UpToDate { checked_at: kit::unix_now() });
    }

    fn update_prefs(&self, change: impl FnOnce(&mut UpdatePrefs)) {
        {
            let mut prefs = lock(&self.shared.prefs);
            change(&mut prefs);
            if let Err(e) = prefs.save(&self.cfg.data_dir) {
                log_line(&self.cfg.data_dir, &format!("Couldn't save the update settings: {e}"));
            }
        }
        self.send(Cmd::Reschedule);
    }

    /// Shows a made-up status (screenshots / demo mode; don't start the worker).
    pub fn set_status_for_demo(&self, status: Status) {
        self.shared.set(status);
    }

    /// Asks the worker to stop and waits (bounded) until it is idle.
    fn pause_worker(&self, wait: Duration) -> Option<MutexGuard<'_, ()>> {
        self.shared.cancel.store(true, AtomicOrdering::SeqCst);
        let deadline = Instant::now() + wait;
        loop {
            match self.shared.work.try_lock() {
                Ok(guard) => return Some(guard),
                Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
                Err(TryLockError::WouldBlock) => {}
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// "Restart now": swaps in the verified update and starts it with
    /// `--updated-from <this version>` plus `extra_args`. On success the
    /// caller saves its state and exits right away.
    pub fn install_and_restart(&self, extra_args: &[String]) -> Result<(), String> {
        let Status::Ready { release, path } = self.status() else {
            return Err("No update is ready to install yet".to_owned());
        };
        let _idle = self.pause_worker(Duration::from_secs(10));
        self.shared.set(Status::Installing);
        let result = (|| {
            let exe = std::env::current_exe().map_err(|e| format!("Couldn't find the running program ({e})"))?;
            // The file waited in a user-writable folder: check it again.
            verify_file(&path, release.asset_size, release.sha256.as_deref(), EXE_MAGIC)?;
            replace_exe(&exe, &path)?;
            let mut cmd = std::process::Command::new(&exe);
            cmd.arg("--updated-from")
                .arg(self.cfg.current_version)
                .args(extra_args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Some(dir) = exe.parent() {
                cmd.current_dir(dir);
            }
            cmd.spawn().map(drop).map_err(|e| {
                restore_exe(&exe);
                format!("Couldn't start the new version ({e})")
            })
        })();
        match &result {
            Ok(()) => log_line(&self.cfg.data_dir, &format!("Installed {}; restarting", release.version)),
            Err(message) => {
                log_line(&self.cfg.data_dir, &format!("Installing {} failed: {message}", release.version));
                self.shared.cancel.store(false, AtomicOrdering::SeqCst);
                self.shared.set(Status::Failed { message: message.clone(), release: Some(release) });
            }
        }
        result
    }

    /// Call from the app's exit path after its state is saved: if an update
    /// is ready (and auto-install is on, or the user asked for it), swap it in
    /// without restarting, so the next start runs the new version.
    pub fn install_on_exit(&self) -> bool {
        let Status::Ready { release, path } = self.status() else { return false };
        if !(self.prefs().auto_install || self.shared.requested.load(AtomicOrdering::SeqCst)) {
            return false;
        }
        let _idle = self.pause_worker(Duration::from_secs(5));
        self.shared.set(Status::Installing);
        let result = std::env::current_exe()
            .map_err(|e| format!("Couldn't find the running program ({e})"))
            .and_then(|exe| {
                verify_file(&path, release.asset_size, release.sha256.as_deref(), EXE_MAGIC)?;
                replace_exe(&exe, &path)
            });
        match result {
            Ok(()) => {
                log_line(&self.cfg.data_dir, &format!("Installed {} on exit", release.version));
                true
            }
            Err(message) => {
                log_line(&self.cfg.data_dir, &format!("Installing {} on exit failed: {message}", release.version));
                self.shared.set(Status::Failed { message, release: Some(release) });
                false
            }
        }
    }

    // -- UI -----------------------------------------------------------------

    fn open_notes(&self, ui: &egui::Ui, release: &ReleaseInfo) {
        let url = if release.html_url.is_empty() { self.cfg.release_page(&release.version) } else { release.html_url.clone() };
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }

    fn installs_on_exit(&self) -> bool {
        self.prefs().auto_install || self.shared.requested.load(AtomicOrdering::SeqCst)
    }

    /// Should the main window show [`Updater::banner_ui`]?
    pub fn banner_visible(&self) -> bool {
        match self.status() {
            Status::Installing => true,
            Status::Failed { release: None, .. } => false,
            s => s.release().is_some_and(|r| self.hidden.as_deref() != Some(r.version.as_str())),
        }
    }

    /// Height of the banner; give it a top panel of exactly this size.
    pub const BANNER_HEIGHT: f32 = 38.0;

    /// The slim update bar for the main window (fills the `ui` it is given:
    /// use a top panel of [`Updater::BANNER_HEIGHT`] without a frame).
    pub fn banner_ui(&mut self, ui: &mut egui::Ui, p: &Palette) -> Option<UpdateAction> {
        let status = self.status();
        let mut action = None;
        let (tint, icon) = match &status {
            Status::Ready { .. } => (p.success, ph::SEAL_CHECK),
            Status::Failed { .. } => (p.warning, ph::WARNING_CIRCLE),
            Status::Downloading { .. } | Status::Installing => (p.accent2, ph::DOWNLOAD_SIMPLE),
            _ => (p.accent, ph::ARROW_CIRCLE_UP),
        };
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, mix(p.bg, tint, 0.10));
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, alpha(tint, 0.35)));
        let inner = rect.shrink2(vec2(14.0, 5.0));
        let layout = egui::Layout::right_to_left(egui::Align::Center);
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(layout), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let link = |ui: &mut egui::Ui, text: &str| {
                ui.add(egui::Button::new(RichText::new(text).size(12.5).color(p.text_dim)).frame(false))
            };
            // Buttons, right to left.
            let mut title = String::new();
            let mut detail = String::new();
            let mut progress: Option<(f32, String)> = None;
            match &status {
                Status::Available(r) => {
                    if link(ui, "Skip this version").on_hover_text("Don't offer this version again").clicked() {
                        self.skip_version();
                    }
                    if link(ui, "Later").on_hover_text("Hide until the app restarts").clicked() {
                        self.hidden = Some(r.version.clone());
                    }
                    if ui.button("What's new").clicked() {
                        self.open_notes(ui, r);
                    }
                    if kit::primary_button(ui, p, format!("{}  Update now", ph::DOWNLOAD_SIMPLE)).clicked() {
                        self.download();
                    }
                    title = format!("{} {} is available", self.cfg.app_name, r.version);
                    detail = format!("You have {}", self.cfg.current_version);
                }
                Status::Downloading { release: r, received, total } => {
                    if link(ui, "Later").on_hover_text("Hide; the download continues").clicked() {
                        self.hidden = Some(r.version.clone());
                    }
                    if ui.button("What's new").clicked() {
                        self.open_notes(ui, r);
                    }
                    let frac = if *total > 0 { *received as f32 / *total as f32 } else { 0.0 };
                    let text = if *total > 0 {
                        format!("{} of {}", kit::human_bytes(*received), kit::human_bytes(*total))
                    } else {
                        kit::human_bytes(*received)
                    };
                    progress = Some((frac, text));
                    title = format!("Downloading {} {}…", self.cfg.app_name, r.version);
                }
                Status::Ready { release: r, .. } => {
                    if link(ui, "Later").clicked() {
                        self.hidden = Some(r.version.clone());
                    }
                    if ui.button("What's new").clicked() {
                        self.open_notes(ui, r);
                    }
                    if kit::primary_button(ui, p, format!("{}  Restart now", ph::ARROWS_CLOCKWISE)).clicked() {
                        action = Some(UpdateAction::RestartNow);
                    }
                    title = format!("{} {} is ready", self.cfg.app_name, r.version);
                    detail = if self.installs_on_exit() {
                        "It installs automatically when you close the app.".to_owned()
                    } else {
                        "Restart the app to finish updating.".to_owned()
                    };
                }
                Status::Failed { message, release: Some(r) } => {
                    if link(ui, "Later").clicked() {
                        self.hidden = Some(r.version.clone());
                    }
                    if ui.button(format!("{}  Download from website", ph::ARROW_SQUARE_OUT)).clicked() {
                        self.open_notes(ui, r);
                    }
                    if ui.button(format!("{}  Try again", ph::ARROWS_CLOCKWISE)).clicked() {
                        self.download();
                    }
                    title = format!("Couldn't update to {}", r.version);
                    detail = message.clone();
                }
                Status::Installing => {
                    title = "Installing the update…".to_owned();
                }
                _ => {}
            }
            // What it is about, left to right in the remaining space.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if matches!(status, Status::Installing) {
                    ui.add(egui::Spinner::new().size(16.0).color(tint));
                } else {
                    ui.label(RichText::new(icon).size(18.0).color(tint));
                }
                ui.add(egui::Label::new(RichText::new(&title).strong().color(p.text)).truncate());
                if let Some((frac, text)) = progress {
                    let width = (ui.available_width() - 110.0).clamp(60.0, 220.0);
                    kit::progress_bar(ui, p, frac, tint, width, 6.0);
                    ui.label(RichText::new(text).size(12.5).color(p.text_dim));
                }
                if !detail.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&detail).size(12.5).color(p.text_dim)).truncate())
                        .on_hover_text(&detail);
                }
            });
        });
        action
    }

    /// The body of the "Updates" section in Settings › About (the app draws
    /// the section title): version, status, "Check for updates" and the two
    /// automatic-update switches.
    pub fn settings_ui(&mut self, ui: &mut egui::Ui, p: &Palette) -> Option<UpdateAction> {
        let status = self.status();
        let prefs = self.prefs();
        let mut action = None;
        kit::card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (icon, color, headline): (&str, Color32, String) = match &status {
                Status::Idle if prefs.auto_check => (ph::CLOCK, p.text, "Updates are checked automatically".to_owned()),
                Status::Idle => (ph::CLOCK, p.text, "Automatic update checks are off".to_owned()),
                Status::Checking => ("", p.text, "Checking for updates…".to_owned()),
                Status::UpToDate { .. } => (ph::CHECK_CIRCLE, p.success, format!("{} is up to date", self.cfg.app_name)),
                Status::Available(r) => (ph::ARROW_CIRCLE_UP, p.accent, format!("Version {} is available", r.version)),
                Status::Downloading { release: r, .. } => {
                    (ph::DOWNLOAD_SIMPLE, p.accent2, format!("Downloading version {}…", r.version))
                }
                Status::Ready { release: r, .. } => (ph::SEAL_CHECK, p.success, format!("Version {} is ready to install", r.version)),
                Status::Installing => ("", p.text, "Installing the update…".to_owned()),
                Status::Failed { release: Some(r), .. } => (ph::WARNING_CIRCLE, p.warning, format!("Couldn't update to {}", r.version)),
                Status::Failed { release: None, .. } => (ph::WARNING_CIRCLE, p.warning, "Couldn't check for updates".to_owned()),
            };
            ui.horizontal(|ui| {
                if icon.is_empty() {
                    ui.add(egui::Spinner::new().size(15.0).color(p.accent));
                } else {
                    ui.label(RichText::new(icon).size(17.0).color(color));
                }
                ui.label(RichText::new(headline).strong().color(color));
            });
            let checked = if prefs.last_check > 0 { format!("last checked {}", ago(prefs.last_check)) } else { "not checked yet".to_owned() };
            ui.label(
                RichText::new(format!("Installed version {}  ·  {checked}", self.cfg.current_version))
                    .size(12.5)
                    .color(p.text_dim),
            );
            match &status {
                Status::Downloading { received, total, .. } => {
                    ui.add_space(4.0);
                    let frac = if *total > 0 { *received as f32 / *total as f32 } else { 0.0 };
                    kit::progress_bar(ui, p, frac, p.accent2, ui.available_width(), 6.0);
                    ui.label(
                        RichText::new(format!("{} of {}", kit::human_bytes(*received), kit::human_bytes(*total)))
                            .size(12.0)
                            .color(p.text_dim),
                    );
                }
                Status::Failed { message, .. } => {
                    ui.label(RichText::new(message).size(12.5).color(p.text_dim));
                }
                Status::Available(r) | Status::Ready { release: r, .. } => {
                    if let Status::Ready { .. } = status {
                        let note = if self.installs_on_exit() {
                            "It installs automatically when you close the app, or restart now."
                        } else {
                            "Restart the app to finish updating."
                        };
                        ui.label(RichText::new(note).size(12.5).color(p.text_dim));
                    }
                    let preview = notes_preview(&r.notes, 6);
                    if !preview.is_empty() {
                        ui.add_space(4.0);
                        egui::Frame::new()
                            .fill(mix(p.surface, p.bg, 0.5))
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(preview).size(12.5).color(p.text_dim));
                            });
                    }
                }
                _ => {}
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                match &status {
                    Status::Available(_) => {
                        if kit::primary_button(ui, p, format!("{}  Update now", ph::DOWNLOAD_SIMPLE)).clicked() {
                            self.download();
                        }
                    }
                    Status::Ready { .. } => {
                        if kit::primary_button(ui, p, format!("{}  Restart now", ph::ARROWS_CLOCKWISE)).clicked() {
                            action = Some(UpdateAction::RestartNow);
                        }
                    }
                    Status::Failed { release: Some(r), .. } => {
                        if ui.button(format!("{}  Try again", ph::ARROWS_CLOCKWISE)).clicked() {
                            self.download();
                        }
                        if ui.button(format!("{}  Download from website", ph::ARROW_SQUARE_OUT)).clicked() {
                            self.open_notes(ui, r);
                        }
                    }
                    _ => {}
                }
                if let Some(r) = status.release()
                    && !matches!(status, Status::Failed { .. })
                    && ui.button("What's new").clicked()
                {
                    self.open_notes(ui, r);
                }
                let check = ui.add_enabled(
                    !status.busy() && self.tx.is_some(),
                    egui::Button::new(format!("{}  Check for updates", ph::ARROWS_CLOCKWISE)),
                );
                if check.clicked() {
                    self.check_now();
                }
            });
            ui.add_space(6.0);
            let mut auto_check = prefs.auto_check;
            if ui.checkbox(&mut auto_check, "Check for updates automatically").changed() {
                self.update_prefs(|p| p.auto_check = auto_check);
            }
            let mut auto_install = prefs.auto_install;
            if ui
                .checkbox(&mut auto_install, "Download updates in the background and install them when I close the app")
                .changed()
            {
                self.update_prefs(|p| p.auto_install = auto_install);
            }
            if let Some(v) = &prefs.skipped_version {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Version {v} is skipped.")).size(12.5).color(p.text_dim));
                    if ui.small_button("Don't skip").clicked() {
                        self.update_prefs(|p| p.skipped_version = None);
                    }
                });
            }
        });
        action
    }
}

// ---------------------------------------------------------------------------
// Worker thread
// ---------------------------------------------------------------------------

/// When the next scheduled check is due.
fn next_check(prefs: &UpdatePrefs, delay: Duration, ignore_last: bool) -> Option<Instant> {
    if !prefs.auto_check {
        return None;
    }
    let now = Instant::now();
    if ignore_last {
        return Some(now + delay);
    }
    let age = Duration::from_secs(kit::unix_now().saturating_sub(prefs.last_check));
    Some(now + if age >= CHECK_EVERY { delay } else { (CHECK_EVERY - age).max(delay) })
}

fn worker(cfg: Arc<UpdaterConfig>, shared: Arc<Shared>, api: String, rx: Receiver<Cmd>, mut hook: Option<CheckHook>) {
    cleanup_leftovers(&cfg);
    let forced = std::env::var("ZENLESS_UPDATE_DELAY_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let mut next = {
        let prefs = lock(&shared.prefs);
        next_check(&prefs, forced.unwrap_or(FIRST_CHECK_AFTER), forced.is_some())
    };
    loop {
        let msg = match next {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let auto = lock(&shared.prefs).auto_check;
        match msg {
            Ok(Cmd::Check { manual }) => {
                run_check(&cfg, &shared, &api, hook.as_mut(), manual);
                next = auto.then(|| Instant::now() + CHECK_EVERY);
            }
            Ok(Cmd::Download) => {
                let _work = lock(&shared.work);
                download_locked(&cfg, &shared);
            }
            Ok(Cmd::Reschedule) => {
                if !auto {
                    next = None;
                } else if next.is_none() {
                    next = next_check(&lock(&shared.prefs), Duration::from_secs(2), false);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if auto {
                    run_check(&cfg, &shared, &api, hook.as_mut(), false);
                }
                next = auto.then(|| Instant::now() + CHECK_EVERY);
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Deletes what the previous update left behind: partial downloads,
/// installers of versions that are already installed, and `<exe>.old`.
fn cleanup_leftovers(cfg: &UpdaterConfig) {
    if let Ok(entries) = fs::read_dir(cfg.updates_dir()) {
        let prefix = format!("{}-", cfg.app_id);
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stale = name.ends_with(".part")
                || name
                    .strip_prefix(&prefix)
                    .and_then(|n| n.strip_suffix(".exe"))
                    .is_some_and(|v| !is_newer(v, cfg.current_version));
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let old = old_exe_path(&exe);
    if !old.exists() {
        return;
    }
    // The previous version may still be shutting down: retry for a minute.
    let data_dir = cfg.data_dir.clone();
    let _ = std::thread::Builder::new().name("updater-cleanup".into()).spawn(move || {
        for _ in 0..60 {
            if fs::remove_file(&old).is_ok() || !old.exists() {
                log_line(&data_dir, &format!("Removed {}", old.display()));
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        log_line(&data_dir, &format!("Couldn't remove {}", old.display()));
    });
}

fn run_check(cfg: &UpdaterConfig, shared: &Shared, api: &str, hook: Option<&mut CheckHook>, manual: bool) {
    let _work = lock(&shared.work);
    if shared.cancelled() {
        return;
    }
    let prev = shared.status();
    let mut download_next = false;
    if !matches!(prev, Status::Downloading { .. } | Status::Installing) {
        if !matches!(prev, Status::Ready { .. }) {
            shared.set(Status::Checking);
        }
        let result = fetch_latest(api, cfg.repo, cfg.asset, &cfg.user_agent());
        let now = kit::unix_now();
        let prefs = {
            let mut p = lock(&shared.prefs);
            p.last_check = now;
            let _ = p.save(&cfg.data_dir);
            p.clone()
        };
        let next = match result {
            Err(message) => {
                log_line(&cfg.data_dir, &format!("Update check failed: {message}"));
                match prev {
                    Status::Ready { .. } => prev,
                    _ => Status::Failed { message, release: None },
                }
            }
            Ok(rel) if !is_newer(&rel.version, cfg.current_version) => {
                log_line(&cfg.data_dir, &format!("Up to date ({} is the latest release)", rel.version));
                Status::UpToDate { checked_at: now }
            }
            Ok(rel) if prefs.skipped_version.as_deref() == Some(rel.version.as_str()) => {
                log_line(&cfg.data_dir, &format!("{} is available but skipped", rel.version));
                Status::UpToDate { checked_at: now }
            }
            Ok(rel) => match prev {
                Status::Ready { ref release, .. } if release.version == rel.version => prev,
                _ if !rel.has_asset() => {
                    let message = format!("Release {} has no {} download", rel.tag, cfg.asset);
                    log_line(&cfg.data_dir, &message);
                    Status::Failed { message, release: Some(rel) }
                }
                _ => {
                    log_line(&cfg.data_dir, &format!("{} is available", rel.version));
                    download_next = prefs.auto_install;
                    Status::Available(rel)
                }
            },
        };
        shared.set(next);
    }
    // App-specific extras (e.g. browser extensions) piggyback on every check.
    if let Some(hook) = hook
        && !shared.cancelled()
    {
        let ctx = lock(&shared.ctx).clone();
        hook(&CheckContext { config: cfg, api, manual, ctx: ctx.as_ref(), cancel: &shared.cancel });
    }
    if download_next {
        download_locked(cfg, shared);
    }
}

/// Downloads the available release. The caller holds `shared.work`.
fn download_locked(cfg: &UpdaterConfig, shared: &Shared) {
    let release = match shared.status() {
        Status::Available(r) | Status::Failed { release: Some(r), .. } => r,
        _ => return,
    };
    if shared.cancelled() {
        return;
    }
    if release.sha256.is_none() {
        log_line(&cfg.data_dir, &format!("{}: {NO_CHECKSUM}", release.version));
        shared.set(Status::Failed { message: NO_CHECKSUM.to_owned(), release: Some(release) });
        return;
    }
    let dest = cfg.updates_dir().join(format!("{}-{}.exe", cfg.app_id, release.version));
    shared.set(Status::Downloading { release: release.clone(), received: 0, total: release.asset_size });
    let mut last = Instant::now();
    let mut progress = |received: u64, total: u64| {
        if last.elapsed() >= Duration::from_millis(100) || received == total {
            last = Instant::now();
            shared.set(Status::Downloading { release: release.clone(), received, total });
        }
        !shared.cancelled()
    };
    let result = download_verified(
        &release.asset_url,
        &cfg.user_agent(),
        release.asset_size,
        release.sha256.as_deref(),
        EXE_MAGIC,
        &dest,
        &mut progress,
    );
    match result {
        Ok(()) => {
            log_line(&cfg.data_dir, &format!("Downloaded and verified {} ({})", release.version, dest.display()));
            shared.set(Status::Ready { release, path: dest });
        }
        Err(message) => {
            log_line(&cfg.data_dir, &format!("Downloading {} failed: {message}", release.version));
            if !shared.cancelled() {
                shared.set(Status::Failed { message, release: Some(release) });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zenless-updater-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(data: &[u8]) -> String {
        to_hex(&Sha256::digest(data))
    }

    /// A tiny_http server answering fixed paths.
    fn serve(routes: HashMap<String, Vec<u8>>, server: tiny_http::Server) {
        std::thread::spawn(move || {
            for rq in server.incoming_requests() {
                let path = rq.url().to_owned();
                let _ = match routes.get(&path) {
                    Some(body) => rq.respond(tiny_http::Response::from_data(body.clone())),
                    None => rq.respond(tiny_http::Response::empty(404)),
                };
            }
        });
    }

    fn bind() -> (tiny_http::Server, String) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        (server, format!("http://127.0.0.1:{port}"))
    }

    /// GitHub-shaped release JSON for `asset` (digest optional, `.sha256` sibling optional).
    fn release_json(base: &str, version: &str, asset: &str, data: &[u8], digest: Option<String>, sha_file: bool) -> String {
        let mut assets = vec![serde_json::json!({
            "name": asset,
            "size": data.len(),
            "digest": digest,
            "content_type": "application/octet-stream",
            "browser_download_url": format!("{base}/dl/{asset}"),
        })];
        if sha_file {
            assets.push(serde_json::json!({
                "name": format!("{asset}.sha256"),
                "size": 81,
                "digest": null,
                "browser_download_url": format!("{base}/dl/{asset}.sha256"),
            }));
        }
        serde_json::json!({
            "tag_name": format!("v{version}"),
            "name": format!("Test {version}"),
            "html_url": format!("https://github.com/zenless-inc/test-app/releases/tag/v{version}"),
            "body": "## What's new\n* Faster\n* Better",
            "published_at": "2026-10-01T12:00:00Z",
            "assets": assets,
        })
        .to_string()
    }

    fn asset_bytes() -> Vec<u8> {
        let mut data = b"MZ\x90\x00".to_vec();
        data.extend((0..300_000u32).map(|i| (i * 7 % 251) as u8));
        data
    }

    fn config(dir: &Path) -> UpdaterConfig {
        UpdaterConfig {
            app_id: "zenless-test",
            app_name: "Zenless Test",
            repo: "zenless-inc/test-app",
            asset: "zenless-test.exe",
            current_version: "0.1.0",
            data_dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.0"), "numeric, not lexicographic");
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("1.0.0", "1.0.0-beta"), "a pre-release sorts before the plain version");
        assert!(!is_newer("1.0.0-beta", "1.0.0"));
        assert!(is_newer("1.0.0-beta.11", "1.0.0-beta.2"));
        assert!(is_newer("1.0.0-rc.1", "1.0.0-beta.9"));
        assert!(!is_newer("garbage", "0.1.0"), "unparsable → not newer");
        assert!(!is_newer("1.0.0", "x.y"));
        assert!(!is_newer("1.2.3.4", "0.1.0"));
        assert_eq!(Version::parse("1.2"), Version::parse("1.2.0"));
        assert_eq!(Version::parse("1.2.3+build.9"), Version::parse("1.2.3"));
        assert!(Version::parse("1.0.0-").is_none());
        assert!(Version::parse("").is_none());
    }

    #[test]
    fn tag_and_digest_parsing() {
        assert_eq!(version_from_tag("v1.2.3"), "1.2.3");
        assert_eq!(version_from_tag(" V0.1.0 "), "0.1.0");
        assert_eq!(version_from_tag("0.1.0"), "0.1.0");
        let hex = "9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08";
        assert_eq!(parse_digest(&format!("sha256:{hex}")), Some(hex.to_ascii_lowercase()));
        assert_eq!(parse_digest("sha512:abcd"), None);
        assert_eq!(parse_digest("sha256:1234"), None, "too short");
        assert_eq!(parse_digest(hex), None, "no algorithm");
        assert_eq!(parse_sha256_file(&format!("{hex}  zenless-dm.exe\n")), Some(hex.to_ascii_lowercase()));
        assert_eq!(parse_sha256_file(&format!("{hex} *zenless-dm.exe")), Some(hex.to_ascii_lowercase()));
        assert_eq!(parse_sha256_file("not a checksum"), None);
        assert_eq!(parse_sha256_file(""), None);
    }

    /// Trimmed copy of a real `GET /repos/{owner}/{repo}/releases/latest` answer.
    const GITHUB_SAMPLE: &str = r###"{
      "url": "https://api.github.com/repos/zenless-inc/zenless-download-manager/releases/250123456",
      "assets_url": "https://api.github.com/repos/zenless-inc/zenless-download-manager/releases/250123456/assets",
      "upload_url": "https://uploads.github.com/repos/zenless-inc/zenless-download-manager/releases/250123456/assets{?name,label}",
      "html_url": "https://github.com/zenless-inc/zenless-download-manager/releases/tag/v0.2.0",
      "id": 250123456,
      "author": { "login": "github-actions[bot]", "id": 41898282, "type": "Bot", "site_admin": false },
      "node_id": "RE_kwDOPx1234c4O6abc",
      "tag_name": "v0.2.0",
      "target_commitish": "main",
      "name": "Zenless Download Manager 0.2.0",
      "draft": false,
      "immutable": false,
      "prerelease": false,
      "created_at": "2026-10-01T10:02:11Z",
      "updated_at": "2026-10-01T10:09:40Z",
      "published_at": "2026-10-01T10:09:40Z",
      "assets": [
        {
          "url": "https://api.github.com/repos/zenless-inc/zenless-download-manager/releases/assets/300000001",
          "id": 300000001,
          "node_id": "RA_kwDOPx1234c4R4abc",
          "name": "zenless-dm.exe",
          "label": "",
          "uploader": { "login": "github-actions[bot]", "id": 41898282, "type": "Bot" },
          "content_type": "application/x-msdownload",
          "state": "uploaded",
          "size": 9864704,
          "digest": "sha256:2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae",
          "download_count": 17,
          "created_at": "2026-10-01T10:09:38Z",
          "updated_at": "2026-10-01T10:09:39Z",
          "browser_download_url": "https://github.com/zenless-inc/zenless-download-manager/releases/download/v0.2.0/zenless-dm.exe"
        },
        {
          "url": "https://api.github.com/repos/zenless-inc/zenless-download-manager/releases/assets/300000002",
          "id": 300000002,
          "name": "zenless-dm.exe.sha256",
          "label": null,
          "content_type": "text/plain",
          "state": "uploaded",
          "size": 81,
          "digest": "sha256:fcde2b2edba56bf408601fb721fe9b5c338d10ee429ea04fae5511b68fbf8fb9",
          "download_count": 3,
          "browser_download_url": "https://github.com/zenless-inc/zenless-download-manager/releases/download/v0.2.0/zenless-dm.exe.sha256"
        }
      ],
      "tarball_url": "https://api.github.com/repos/zenless-inc/zenless-download-manager/tarball/v0.2.0",
      "zipball_url": "https://api.github.com/repos/zenless-inc/zenless-download-manager/zipball/v0.2.0",
      "body": "## What's Changed\r\n* Automatic updates by @zenless in https://github.com/zenless-inc/zenless-download-manager/pull/3\r\n\r\n**Full Changelog**: https://github.com/zenless-inc/zenless-download-manager/compare/v0.1.0...v0.2.0",
      "mentions_count": 1
    }"###;

    #[test]
    fn github_json_parsing() {
        let parsed = parse_release(GITHUB_SAMPLE, "zenless-dm.exe").unwrap();
        let r = &parsed.info;
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.tag, "v0.2.0");
        assert_eq!(r.html_url, "https://github.com/zenless-inc/zenless-download-manager/releases/tag/v0.2.0");
        assert_eq!(
            r.asset_url,
            "https://github.com/zenless-inc/zenless-download-manager/releases/download/v0.2.0/zenless-dm.exe"
        );
        assert_eq!(r.asset_size, 9_864_704);
        assert_eq!(r.sha256.as_deref(), Some("2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"));
        assert_eq!(r.published_at, "2026-10-01T10:09:40Z");
        assert!(r.notes.starts_with("## What's Changed"));
        assert!(parsed.checksum_url.as_deref().is_some_and(|u| u.ends_with("/zenless-dm.exe.sha256")));
        let preview = notes_preview(&r.notes, 2);
        assert!(preview.starts_with("What's Changed\n•  Automatic updates"), "{preview}");
        assert!(preview.ends_with(" …"));

        // The asset must match exactly; `null` fields are fine.
        let other = parse_release(GITHUB_SAMPLE, "zenless-torrent.exe").unwrap();
        assert!(!other.info.has_asset());
        assert_eq!(other.info.sha256, None);
        let minimal = parse_release(r#"{"tag_name":"v1.0.0","body":null,"html_url":null,"assets":[]}"#, "x.exe").unwrap();
        assert_eq!(minimal.info.version, "1.0.0");
        assert!(minimal.info.notes.is_empty());
        assert!(parse_release("<html>rate limited</html>", "x.exe").is_err());
    }

    #[test]
    fn prefs_defaults_and_round_trip() {
        let dir = temp_dir("prefs");
        let p = UpdatePrefs::load(&dir);
        assert!(p.auto_check && p.auto_install && p.skipped_version.is_none() && p.last_check == 0);
        let p = UpdatePrefs { auto_install: false, skipped_version: Some("0.3.0".into()), last_check: 42, ..p };
        p.save(&dir).unwrap();
        assert_eq!(UpdatePrefs::load(&dir), p);
        fs::write(dir.join(PREFS_FILE), r#"{"auto_check":false,"future":1}"#).unwrap();
        let p = UpdatePrefs::load(&dir);
        assert!(!p.auto_check && p.auto_install, "unknown/missing fields are tolerated");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_mismatch_and_bad_files_are_rejected() {
        let data = asset_bytes();
        let (server, base) = bind();
        let mut routes = HashMap::new();
        routes.insert("/dl/app.exe".to_owned(), data.clone());
        routes.insert("/dl/notes.txt".to_owned(), b"hello, not a program".to_vec());
        serve(routes, server);
        let dir = temp_dir("mismatch");
        let dest = dir.join("app-0.2.0.exe");
        let mut ok = |_: u64, _: u64| true;
        let url = format!("{base}/dl/app.exe");

        let wrong = sha(b"something else");
        let err = download_verified(&url, "test/1", data.len() as u64, Some(&wrong), EXE_MAGIC, &dest, &mut ok).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(!dest.exists() && !with_suffix(&dest, ".part").exists(), "nothing is left behind");

        let err = download_verified(&url, "test/1", data.len() as u64, None, EXE_MAGIC, &dest, &mut ok).unwrap_err();
        assert_eq!(err, NO_CHECKSUM);

        let right = sha(&data);
        let err = download_verified(&url, "test/1", data.len() as u64 + 1, Some(&right), EXE_MAGIC, &dest, &mut ok).unwrap_err();
        assert!(err.contains("incomplete"), "{err}");

        let txt = b"hello, not a program";
        let err = download_verified(&format!("{base}/dl/notes.txt"), "test/1", txt.len() as u64, Some(&sha(txt)), EXE_MAGIC, &dest, &mut ok)
            .unwrap_err();
        assert!(err.contains("format"), "{err}");

        let mut seen = 0;
        let mut count = |r: u64, _: u64| {
            seen = r;
            true
        };
        download_verified(&url, "test/1", data.len() as u64, Some(&right), EXE_MAGIC, &dest, &mut count).unwrap();
        assert_eq!(seen, data.len() as u64);
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(verify_file(&dest, data.len() as u64, Some(&right), EXE_MAGIC).is_ok());
        assert!(verify_file(&dest, 0, Some(&wrong), EXE_MAGIC).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_file_fallback() {
        let data = asset_bytes();
        let (server, base) = bind();
        let mut routes = HashMap::new();
        routes.insert(
            "/repos/zenless-inc/test-app/releases/latest".to_owned(),
            release_json(&base, "0.2.0", "zenless-test.exe", &data, None, true).into_bytes(),
        );
        routes.insert(
            "/dl/zenless-test.exe.sha256".to_owned(),
            format!("{}  zenless-test.exe\n", sha(&data).to_ascii_uppercase()).into_bytes(),
        );
        serve(routes, server);
        let rel = fetch_latest(&base, "zenless-inc/test-app", "zenless-test.exe", "test/1").unwrap();
        assert_eq!(rel.version, "0.2.0");
        assert_eq!(rel.sha256, Some(sha(&data)));
        assert_eq!(rel.asset_url, format!("{base}/dl/zenless-test.exe"));
        let err = fetch_latest(&base, "zenless-inc/missing", "zenless-test.exe", "test/1").unwrap_err();
        assert!(err.contains("No release"), "{err}");
    }

    #[test]
    fn replace_exe_keeps_the_old_one_and_rolls_back() {
        let dir = temp_dir("replace");
        let exe = dir.join("app.exe");
        let new = dir.join("new.exe");
        fs::write(&exe, b"MZ old").unwrap();
        fs::write(&new, b"MZ new").unwrap();
        replace_exe(&exe, &new).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"MZ new");
        assert_eq!(fs::read(old_exe_path(&exe)).unwrap(), b"MZ old");
        // A second update replaces the stale `.old`.
        fs::write(&new, b"MZ newer").unwrap();
        replace_exe(&exe, &new).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"MZ newer");
        assert_eq!(fs::read(old_exe_path(&exe)).unwrap(), b"MZ new");
        // A missing source rolls back to the running version.
        let err = replace_exe(&exe, &dir.join("missing.exe")).unwrap_err();
        assert!(err.starts_with("Zenless can't replace itself in"), "{err}");
        assert_eq!(fs::read(&exe).unwrap(), b"MZ newer");
        let _ = fs::remove_dir_all(&dir);
    }

    fn wait_for(updater: &Updater, done: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let s = updater.status();
            if done(&s) || Instant::now() > deadline {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Mock GitHub: check → download → verify → Ready; then a tampered asset.
    #[test]
    fn check_download_verify_with_mock_server() {
        let data = asset_bytes();
        let tampered = {
            let mut t = data.clone();
            let n = t.len();
            t[n / 2] ^= 0xff;
            t
        };
        let (server, base) = bind();
        let mut routes = HashMap::new();
        let digest = Some(format!("sha256:{}", sha(&data)));
        routes.insert(
            "/good/repos/zenless-inc/test-app/releases/latest".to_owned(),
            release_json(&format!("{base}/good"), "0.2.0", "zenless-test.exe", &data, digest.clone(), false).into_bytes(),
        );
        routes.insert("/good/dl/zenless-test.exe".to_owned(), data.clone());
        routes.insert(
            "/bad/repos/zenless-inc/test-app/releases/latest".to_owned(),
            release_json(&format!("{base}/bad"), "0.2.0", "zenless-test.exe", &data, digest, false).into_bytes(),
        );
        routes.insert("/bad/dl/zenless-test.exe".to_owned(), tampered);
        serve(routes, server);
        let ctx = egui::Context::default();

        // Good release, downloaded in the background (auto_install is on by default).
        let dir = temp_dir("mock-good");
        UpdatePrefs { auto_check: false, ..Default::default() }.save(&dir).unwrap();
        let hook_calls = Arc::new(Mutex::new(Vec::new()));
        let calls = hook_calls.clone();
        let mut updater = Updater::new(config(&dir)).with_api(format!("{base}/good"));
        updater.on_check(move |cx| calls.lock().unwrap().push((cx.manual, cx.api.to_owned())));
        updater.start(&ctx);
        updater.check_now();
        let status = wait_for(&updater, |s| matches!(s, Status::Ready { .. } | Status::Failed { .. }));
        let Status::Ready { release, path } = status else { panic!("expected Ready, got {status:?}") };
        assert_eq!(release.version, "0.2.0");
        assert_eq!(path, dir.join("updates").join("zenless-test-0.2.0.exe"));
        assert_eq!(fs::read(&path).unwrap(), data);
        assert!(updater.prefs().last_check > 0);
        assert_eq!(hook_calls.lock().unwrap().as_slice(), [(true, format!("{base}/good"))]);
        // Skipping turns it into "up to date" (and is remembered).
        updater.skip_version();
        assert!(matches!(updater.status(), Status::UpToDate { .. }));
        assert_eq!(UpdatePrefs::load(&dir).skipped_version.as_deref(), Some("0.2.0"));
        drop(updater);
        let _ = fs::remove_dir_all(&dir);

        // Tampered asset: rejected, nothing kept.
        let dir = temp_dir("mock-bad");
        UpdatePrefs { auto_check: false, ..Default::default() }.save(&dir).unwrap();
        let mut updater = Updater::new(config(&dir)).with_api(format!("{base}/bad"));
        updater.start(&ctx);
        updater.check_now();
        let status = wait_for(&updater, |s| matches!(s, Status::Ready { .. } | Status::Failed { .. }));
        let Status::Failed { message, release } = status else { panic!("expected Failed, got {status:?}") };
        assert!(message.contains("checksum"), "{message}");
        assert_eq!(release.map(|r| r.version).as_deref(), Some("0.2.0"));
        let leftovers: Vec<_> = fs::read_dir(dir.join("updates"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != LOG_FILE)
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        assert!(!updater.install_on_exit(), "nothing to install");
        assert!(updater.install_and_restart(&[]).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn up_to_date_and_missing_checksum() {
        let data = asset_bytes();
        let (server, base) = bind();
        let mut routes = HashMap::new();
        routes.insert(
            "/same/repos/zenless-inc/test-app/releases/latest".to_owned(),
            release_json(&base, "0.1.0", "zenless-test.exe", &data, Some(format!("sha256:{}", sha(&data))), false).into_bytes(),
        );
        routes.insert(
            "/nosum/repos/zenless-inc/test-app/releases/latest".to_owned(),
            release_json(&base, "0.3.0", "zenless-test.exe", &data, None, false).into_bytes(),
        );
        serve(routes, server);
        let ctx = egui::Context::default();
        for (path, expect_failed) in [("same", false), ("nosum", true)] {
            let dir = temp_dir(&format!("mock-{path}"));
            UpdatePrefs { auto_check: false, ..Default::default() }.save(&dir).unwrap();
            let mut updater = Updater::new(config(&dir)).with_api(format!("{base}/{path}"));
            updater.start(&ctx);
            updater.check_now();
            let status = wait_for(&updater, |s| matches!(s, Status::UpToDate { .. } | Status::Failed { .. } | Status::Ready { .. }));
            if expect_failed {
                assert_eq!(status, Status::Failed { message: NO_CHECKSUM.to_owned(), release: status.release().cloned() });
            } else {
                assert!(matches!(status, Status::UpToDate { .. }), "{status:?}");
            }
            drop(updater);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn scheduling() {
        let fresh = UpdatePrefs::default();
        let now = Instant::now();
        let at = next_check(&fresh, FIRST_CHECK_AFTER, false).unwrap();
        assert!(at >= now + FIRST_CHECK_AFTER && at < now + FIRST_CHECK_AFTER + Duration::from_secs(5));
        let recent = UpdatePrefs { last_check: kit::unix_now() - 3600, ..Default::default() };
        let at = next_check(&recent, FIRST_CHECK_AFTER, false).unwrap();
        assert!(at > now + Duration::from_secs(4 * 3600), "checked an hour ago → next in ~5 h");
        assert!(next_check(&recent, Duration::from_secs(2), true).unwrap() < now + Duration::from_secs(5));
        assert!(next_check(&UpdatePrefs { auto_check: false, ..Default::default() }, FIRST_CHECK_AFTER, false).is_none());
    }
}
