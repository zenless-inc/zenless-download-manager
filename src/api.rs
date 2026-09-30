//! Local integration API for the browser extensions (HTTP/1.1 + JSON on
//! `127.0.0.1:6812`), plus a tiny client used for single-instance forwarding.
//!
//! Security rules (see the suite spec):
//! 1. bind 127.0.0.1 only (and reject foreign `Host` headers against DNS rebinding);
//! 2. an `Origin`, when present, must be a browser-extension origin, else 403;
//! 3. every `POST` needs `X-Zenless-Client`, else 400 (forces a CORS preflight for web pages);
//! 4. `OPTIONS` preflight → 204 with CORS headers for allowed origins;
//! 5. `/quit` and local `save_dir` paths only from callers without `Origin`;
//! 6. JSON bodies up to 8 MiB.

use crate::engine::{BatchRequest, DownloadRequest, Engine, Id, NewDownload, Notifier, StartMode, Status, UiEvent};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const PORT: u16 = 6812;
pub const MAX_BODY: usize = 8 * 1024 * 1024;
pub const CLIENT_NAME: &str = concat!("zenless-dm/", env!("CARGO_PKG_VERSION"));

const ALLOWED_ORIGIN_PREFIXES: [&str; 4] = [
    "chrome-extension://",
    "moz-extension://",
    "extension://",
    "safari-web-extension://",
];

/// Is this `Origin` a browser extension?
pub fn origin_allowed(origin: &str) -> bool {
    let o = origin.trim();
    ALLOWED_ORIGIN_PREFIXES
        .iter()
        .any(|p| o.len() > p.len() && o[..p.len()].eq_ignore_ascii_case(p))
}

/// Only accept `Host` headers naming the loopback interface.
pub fn host_allowed(host: &str, port: u16) -> bool {
    let h = host.trim().to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|name| h == *name || h == format!("{name}:{port}"))
}

// ---------------------------------------------------------------------------
// Transport-independent request handling (unit-testable)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ApiRequest {
    pub method: String,
    /// Path including any query string.
    pub url: String,
    pub origin: Option<String>,
    pub client: Option<String>,
    pub host: Option<String>,
    pub body: Vec<u8>,
    pub body_too_large: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Option<Value>,
    /// Echoed in `Access-Control-Allow-Origin`.
    pub cors_origin: Option<String>,
    pub preflight: bool,
}

impl ApiResponse {
    fn json(status: u16, body: Value) -> Self {
        Self { status, body: Some(body), cors_origin: None, preflight: false }
    }
    fn error(status: u16, msg: &str) -> Self {
        Self::json(status, json!({ "ok": false, "error": msg }))
    }
}

/// What the API needs from the application.
pub trait ApiBackend: Send + Sync {
    fn status(&self) -> Value;
    fn add(&self, req: DownloadRequest, mode: StartMode) -> Id;
    fn ask(&self, req: DownloadRequest);
    fn batch(&self, req: BatchRequest);
    fn focus(&self);
    fn quit(&self);
    /// A client talked to us (for "last contact" in the settings).
    fn contact(&self, client: &str);
}

fn valid_download_url(url: &str) -> bool {
    crate::util::looks_like_url(url)
}

/// Applies the security rules and routes the request.
pub fn handle(req: &ApiRequest, backend: &dyn ApiBackend, port: u16) -> ApiResponse {
    if let Some(h) = &req.host
        && !host_allowed(h, port)
    {
        return ApiResponse::error(403, "forbidden host");
    }
    let cors = match &req.origin {
        None => None,
        Some(o) if origin_allowed(o) => Some(o.trim().to_owned()),
        Some(_) => return ApiResponse::error(403, "origin not allowed"),
    };
    let mut resp = route(req, backend, cors.is_some());
    resp.cors_origin = cors;
    resp
}

fn route(req: &ApiRequest, backend: &dyn ApiBackend, has_origin: bool) -> ApiResponse {
    let method = req.method.to_ascii_uppercase();
    let path = req.url.split(['?', '#']).next().unwrap_or("").trim_end_matches('/');
    let path = if path.is_empty() { "/" } else { path };

    if method == "OPTIONS" {
        return ApiResponse { status: 204, body: None, cors_origin: None, preflight: true };
    }
    let client = req.client.as_deref().map(str::trim).filter(|c| !c.is_empty());
    if let Some(c) = client {
        backend.contact(c);
    } else if has_origin {
        backend.contact(req.origin.as_deref().unwrap_or("extension"));
    }
    if method == "POST" && client.is_none() {
        return ApiResponse::error(400, "missing X-Zenless-Client header");
    }
    if req.body_too_large {
        return ApiResponse::error(413, "request body too large (max 8 MiB)");
    }

    match (method.as_str(), path) {
        ("GET", "/ping") => ApiResponse::json(
            200,
            json!({ "ok": true, "app": crate::APP_ID, "name": crate::APP_NAME, "version": crate::VERSION }),
        ),
        ("GET", "/status") => ApiResponse::json(200, backend.status()),
        ("POST", "/download") => {
            let parsed: DownloadRequest = match serde_json::from_slice(&req.body) {
                Ok(r) => r,
                Err(e) => return ApiResponse::error(400, &format!("invalid JSON: {e}")),
            };
            if !valid_download_url(&parsed.url) {
                return ApiResponse::error(400, "a valid http(s) \"url\" is required");
            }
            if parsed.save_dir.is_some() && has_origin {
                return ApiResponse::error(403, "local paths are only accepted from local apps");
            }
            match parsed.mode.as_deref().unwrap_or("ask") {
                "ask" => {
                    backend.ask(parsed);
                    ApiResponse::json(200, json!({ "ok": true }))
                }
                "start" => {
                    let id = backend.add(parsed, StartMode::Now);
                    ApiResponse::json(200, json!({ "ok": true, "id": id.to_string() }))
                }
                "queue" => {
                    let id = backend.add(parsed, StartMode::Queue);
                    ApiResponse::json(200, json!({ "ok": true, "id": id.to_string() }))
                }
                other => ApiResponse::error(400, &format!("unknown mode {other:?} (use ask, start or queue)")),
            }
        }
        ("POST", "/batch") => {
            let mut parsed: BatchRequest = match serde_json::from_slice(&req.body) {
                Ok(r) => r,
                Err(e) => return ApiResponse::error(400, &format!("invalid JSON: {e}")),
            };
            parsed.items.retain(|i| valid_download_url(&i.url));
            let mut seen = std::collections::HashSet::new();
            parsed.items.retain(|i| seen.insert(i.url.clone()));
            let count = parsed.items.len();
            if count == 0 {
                return ApiResponse::error(400, "no valid http(s) URLs in \"items\"");
            }
            backend.batch(parsed);
            ApiResponse::json(200, json!({ "ok": true, "count": count }))
        }
        ("POST", "/focus") => {
            backend.focus();
            ApiResponse::json(200, json!({ "ok": true }))
        }
        ("POST", "/quit") => {
            if has_origin {
                return ApiResponse::error(403, "quit is only accepted from local apps");
            }
            backend.quit();
            ApiResponse::json(200, json!({ "ok": true }))
        }
        (_, "/ping" | "/status" | "/download" | "/batch" | "/focus" | "/quit") => {
            ApiResponse::error(405, "method not allowed")
        }
        _ => ApiResponse::error(404, "not found"),
    }
}

// ---------------------------------------------------------------------------
// The real backend
// ---------------------------------------------------------------------------

/// Last client that talked to the API.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contact {
    pub at: u64,
    pub client: String,
}

/// Shown on the "Browser integration" settings page.
#[derive(Clone, Debug, Default)]
pub struct ApiStatus {
    pub listening: bool,
    pub port: u16,
    pub error: Option<String>,
    pub last_contact: Option<Contact>,
    pub last_extension_contact: Option<Contact>,
    pub requests: u64,
}

pub struct EngineBackend {
    pub engine: Engine,
    pub events: std::sync::mpsc::Sender<UiEvent>,
    pub notify: Notifier,
    pub status: Arc<Mutex<ApiStatus>>,
}

impl EngineBackend {
    fn ui(&self, ev: UiEvent) {
        let _ = self.events.send(ev);
        (self.notify)();
    }
}

/// `/status` body for a snapshot.
pub fn status_json(snap: &crate::engine::Snapshot) -> Value {
    let active = snap.count(|d| d.status.is_active());
    let queued = snap.count(|d| d.status == Status::Queued);
    let completed = snap.count(|d| d.status == Status::Completed);
    let mut recent: Vec<&crate::engine::Download> =
        snap.downloads.iter().filter(|d| d.status != Status::Completed).collect();
    recent.sort_by(|a, b| b.added_at.cmp(&a.added_at).then(b.id.cmp(&a.id)));
    let items: Vec<Value> = recent
        .into_iter()
        .take(8)
        .map(|d| {
            json!({
                "id": d.id.to_string(),
                "name": d.file_name,
                "progress": d.progress(),
                "speed": d.rt.speed,
                "status": d.status.api_name(),
            })
        })
        .collect();
    json!({
        "ok": true,
        "app": crate::APP_ID,
        "version": crate::VERSION,
        "active": active,
        "queued": queued,
        "completed": completed,
        "download_speed": snap.total_speed,
        "items": items,
    })
}

impl ApiBackend for EngineBackend {
    fn status(&self) -> Value {
        status_json(&self.engine.snapshot())
    }

    fn add(&self, r: DownloadRequest, mode: StartMode) -> Id {
        let request = r.request_info();
        self.engine.add(NewDownload {
            url: r.url.trim().to_owned(),
            file_name: r.filename.filter(|f| !f.trim().is_empty()),
            save_dir: r.save_dir,
            category: None,
            connections: None,
            request,
            start: mode,
            size_hint: r.size,
            mime: r.mime,
            source: r.source,
            page_title: r.page_title,
        })
    }

    fn ask(&self, r: DownloadRequest) {
        self.ui(UiEvent::Ask(r));
    }

    fn batch(&self, r: BatchRequest) {
        self.ui(UiEvent::Batch(r));
    }

    fn focus(&self) {
        self.ui(UiEvent::Focus);
    }

    fn quit(&self) {
        self.ui(UiEvent::Quit);
        // Safety net in case the window is wedged: save and exit anyway.
        let engine = self.engine.clone();
        let _ = std::thread::Builder::new().name("zdm-quit-watchdog".into()).spawn(move || {
            std::thread::sleep(Duration::from_secs(8));
            engine.shutdown(Duration::from_secs(4));
            std::process::exit(0);
        });
    }

    fn contact(&self, client: &str) {
        if let Ok(mut s) = self.status.lock() {
            let c = Contact { at: crate::util::unix_now(), client: client.chars().take(80).collect() };
            s.requests += 1;
            let lower = c.client.to_ascii_lowercase();
            if lower.contains("extension") {
                s.last_extension_contact = Some(c.clone());
            }
            s.last_contact = Some(c);
        }
    }
}

// ---------------------------------------------------------------------------
// tiny_http server
// ---------------------------------------------------------------------------

fn header_value(rq: &tiny_http::Request, name: &str) -> Option<String> {
    rq.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_owned())
}

fn to_api_request(rq: &mut tiny_http::Request) -> ApiRequest {
    let mut req = ApiRequest {
        method: rq.method().as_str().to_owned(),
        url: rq.url().to_owned(),
        origin: header_value(rq, "Origin"),
        client: header_value(rq, "X-Zenless-Client"),
        host: header_value(rq, "Host"),
        body: Vec::new(),
        body_too_large: false,
    };
    if rq.body_length().is_some_and(|l| l > MAX_BODY) {
        req.body_too_large = true;
        return req;
    }
    let mut body = Vec::new();
    match rq.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body) {
        Ok(n) if n > MAX_BODY => req.body_too_large = true,
        Ok(_) => req.body = body,
        Err(_) => req.body.clear(),
    }
    req
}

fn header(name: &str, value: &str) -> Option<tiny_http::Header> {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
}

fn respond(rq: tiny_http::Request, resp: ApiResponse) {
    let bytes = resp.body.as_ref().map(|b| b.to_string().into_bytes()).unwrap_or_default();
    let mut out = tiny_http::Response::from_data(bytes).with_status_code(resp.status);
    let mut headers = Vec::new();
    if resp.body.is_some() {
        headers.push(header("Content-Type", "application/json; charset=utf-8"));
    }
    headers.push(header("Cache-Control", "no-store"));
    if let Some(origin) = &resp.cors_origin {
        headers.push(header("Access-Control-Allow-Origin", origin));
        headers.push(header("Vary", "Origin"));
        if resp.preflight {
            headers.push(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"));
            headers.push(header("Access-Control-Allow-Headers", "content-type, x-zenless-client"));
            headers.push(header("Access-Control-Allow-Private-Network", "true"));
            headers.push(header("Access-Control-Max-Age", "600"));
        }
    }
    for h in headers.into_iter().flatten() {
        out.add_header(h);
    }
    let _ = rq.respond(out);
}

/// Binds `127.0.0.1:port` and serves requests on a background thread.
pub fn start(backend: Arc<dyn ApiBackend>, port: u16) -> Result<(), String> {
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("zdm-api".into())
        .spawn(move || {
            for mut rq in server.incoming_requests() {
                let req = to_api_request(&mut rq);
                let resp = handle(&req, backend.as_ref(), port);
                respond(rq, resp);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Minimal client (single-instance forwarding)
// ---------------------------------------------------------------------------

/// Sends one HTTP request to the local API and returns `(status, body)`.
pub fn client_request(port: u16, method: &str, path: &str, body: Option<&str>, timeout: Duration) -> std::io::Result<(u16, String)> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = std::net::TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout.max(Duration::from_secs(1))))?;
    stream.set_write_timeout(Some(timeout.max(Duration::from_secs(1))))?;
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Zenless-Client: {CLIENT_NAME}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| std::io::Error::other("malformed HTTP response"))?;
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_owned()).unwrap_or_default();
    Ok((status, body))
}

/// Is another Zenless Download Manager answering on `port`?
pub fn ping_existing(port: u16, timeout: Duration) -> bool {
    match client_request(port, "GET", "/ping", None, timeout) {
        Ok((200, body)) => serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("app").and_then(Value::as_str).map(|a| a == crate::APP_ID))
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct Mock {
        calls: StdMutex<Vec<String>>,
    }

    impl ApiBackend for Mock {
        fn status(&self) -> Value {
            json!({"ok": true})
        }
        fn add(&self, req: DownloadRequest, mode: StartMode) -> Id {
            self.calls.lock().unwrap().push(format!("add {mode:?} {}", req.url));
            42
        }
        fn ask(&self, req: DownloadRequest) {
            self.calls.lock().unwrap().push(format!("ask {}", req.url));
        }
        fn batch(&self, req: BatchRequest) {
            self.calls.lock().unwrap().push(format!("batch {}", req.items.len()));
        }
        fn focus(&self) {
            self.calls.lock().unwrap().push("focus".into());
        }
        fn quit(&self) {
            self.calls.lock().unwrap().push("quit".into());
        }
        fn contact(&self, _client: &str) {}
    }

    fn req(method: &str, url: &str, origin: Option<&str>, client: Option<&str>, body: &str) -> ApiRequest {
        ApiRequest {
            method: method.into(),
            url: url.into(),
            origin: origin.map(Into::into),
            client: client.map(Into::into),
            host: Some("127.0.0.1:6812".into()),
            body: body.as_bytes().to_vec(),
            body_too_large: false,
        }
    }

    #[test]
    fn origin_rules() {
        assert!(origin_allowed("chrome-extension://abcdefghijklmnop"));
        assert!(origin_allowed("moz-extension://1234-5678"));
        assert!(origin_allowed("extension://x"));
        assert!(origin_allowed("safari-web-extension://x"));
        assert!(!origin_allowed("chrome-extension://"));
        assert!(!origin_allowed("https://evil.example"));
        assert!(!origin_allowed("http://127.0.0.1:6812"));
        assert!(!origin_allowed("null"));
        assert!(!origin_allowed("https://chrome-extension.evil.example"));
    }

    #[test]
    fn host_rules() {
        assert!(host_allowed("127.0.0.1:6812", 6812));
        assert!(host_allowed("localhost:6812", 6812));
        assert!(host_allowed("localhost", 6812));
        assert!(!host_allowed("evil.example:6812", 6812));
        assert!(!host_allowed("127.0.0.1:9999", 6812));
    }

    #[test]
    fn web_origins_are_forbidden() {
        let m = Mock::default();
        let r = handle(&req("GET", "/ping", Some("https://evil.example"), None, ""), &m, PORT);
        assert_eq!(r.status, 403);
        let r = handle(&req("POST", "/download", Some("https://evil.example"), Some("x"), r#"{"url":"https://a.b/c"}"#), &m, PORT);
        assert_eq!(r.status, 403);
        assert!(m.calls.lock().unwrap().is_empty());
        let mut rebind = req("GET", "/status", None, None, "");
        rebind.host = Some("attacker.example:6812".into());
        assert_eq!(handle(&rebind, &m, PORT).status, 403);
    }

    #[test]
    fn extension_origins_get_cors() {
        let m = Mock::default();
        let o = "chrome-extension://abc";
        let r = handle(&req("OPTIONS", "/download", Some(o), None, ""), &m, PORT);
        assert_eq!(r.status, 204);
        assert!(r.preflight);
        assert_eq!(r.cors_origin.as_deref(), Some(o));
        let r = handle(&req("GET", "/ping", Some(o), None, ""), &m, PORT);
        assert_eq!(r.status, 200);
        assert_eq!(r.cors_origin.as_deref(), Some(o));
        assert_eq!(r.body.unwrap()["app"], "zenless-dm");
    }

    #[test]
    fn post_requires_client_header() {
        let m = Mock::default();
        let r = handle(&req("POST", "/focus", None, None, ""), &m, PORT);
        assert_eq!(r.status, 400);
        let r = handle(&req("POST", "/focus", None, Some("test/1"), ""), &m, PORT);
        assert_eq!(r.status, 200);
    }

    #[test]
    fn quit_and_paths_only_without_origin() {
        let m = Mock::default();
        let o = Some("moz-extension://x");
        assert_eq!(handle(&req("POST", "/quit", o, Some("firefox-extension/0.1.0"), ""), &m, PORT).status, 403);
        let body = r#"{"url":"https://a.b/c.zip","save_dir":"C:\\Temp","mode":"start"}"#;
        assert_eq!(handle(&req("POST", "/download", o, Some("firefox-extension/0.1.0"), body), &m, PORT).status, 403);
        assert_eq!(handle(&req("POST", "/download", None, Some("cli"), body), &m, PORT).status, 200);
        assert_eq!(handle(&req("POST", "/quit", None, Some("cli"), ""), &m, PORT).status, 200);
        let calls = m.calls.lock().unwrap();
        assert_eq!(calls.as_slice(), ["add Now https://a.b/c.zip", "quit"]);
    }

    #[test]
    fn download_modes_and_validation() {
        let m = Mock::default();
        let c = Some("chrome-extension/0.1.0");
        let o = Some("chrome-extension://abc");
        let r = handle(&req("POST", "/download", o, c, r#"{"url":"https://a.b/1.zip"}"#), &m, PORT);
        assert_eq!(r.status, 200);
        assert_eq!(r.body.unwrap(), json!({"ok": true}));
        let r = handle(&req("POST", "/download", o, c, r#"{"url":"https://a.b/2.zip","mode":"queue"}"#), &m, PORT);
        assert_eq!(r.body.unwrap()["id"], "42");
        assert_eq!(handle(&req("POST", "/download", o, c, r#"{"url":"javascript:alert(1)"}"#), &m, PORT).status, 400);
        assert_eq!(handle(&req("POST", "/download", o, c, "not json"), &m, PORT).status, 400);
        assert_eq!(handle(&req("POST", "/download", o, c, r#"{"url":"https://a.b","mode":"x"}"#), &m, PORT).status, 400);
        let r = handle(
            &req("POST", "/batch", o, c, r#"{"items":[{"url":"https://a.b/1"},{"url":"nope"},{"url":"https://a.b/1"},{"url":"https://a.b/2"}]}"#),
            &m,
            PORT,
        );
        assert_eq!(r.body.unwrap()["count"], 2);
        assert_eq!(handle(&req("GET", "/download", o, None, ""), &m, PORT).status, 405);
        assert_eq!(handle(&req("GET", "/nope", o, None, ""), &m, PORT).status, 404);
        let mut big = req("POST", "/download", o, c, "");
        big.body_too_large = true;
        assert_eq!(handle(&big, &m, PORT).status, 413);
        let calls = m.calls.lock().unwrap();
        assert_eq!(calls.as_slice(), ["ask https://a.b/1.zip", "add Queue https://a.b/2.zip", "batch 2"]);
    }

    #[test]
    fn null_fields_are_tolerated() {
        let r: DownloadRequest =
            serde_json::from_str(r#"{"url":"https://a.b/c","filename":null,"headers":null,"size":null}"#).unwrap();
        assert_eq!(r.url, "https://a.b/c");
        assert!(r.headers.is_none());
    }
}
