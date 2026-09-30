//! HTTP helpers: client construction, request decoration with the browser's
//! context, `Content-Range` parsing and URL probing.

use super::model::{ProbeInfo, RequestInfo};
use crate::filename;
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode};
use std::time::Duration;

/// Error classification for retries.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpError {
    /// Worth retrying (network hiccup, 5xx, timeout…).
    Retry(String),
    /// Retrying won't help (404, 403, bad URL…).
    Fatal(String),
}

impl HttpError {
    pub fn message(&self) -> &str {
        match self {
            HttpError::Retry(m) | HttpError::Fatal(m) => m,
        }
    }
}

/// Builds the shared client. HTTP/1.1 only so that every connection of a
/// segmented download is a real, separate TCP connection (IDM-style) rather
/// than a multiplexed HTTP/2 stream.
pub fn build_client(timeout: Duration) -> reqwest::Result<Client> {
    Client::builder()
        .http1_only()
        .connect_timeout(timeout)
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(32)
        .tcp_nodelay(true)
        .redirect(reqwest::redirect::Policy::limited(10))
        .referer(false)
        .build()
}

/// Header names we manage ourselves and never take from the browser.
fn is_managed_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "range"
            | "host"
            | "content-length"
            | "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "accept-encoding"
            | "te"
            | "upgrade"
            | "proxy-connection"
            | "if-range"
            | "if-modified-since"
            | "if-none-match"
    )
}

/// Headers for every request of a download: user agent, referrer, cookies and
/// any extra headers from the browser.
pub fn request_headers(info: &RequestInfo, default_ua: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    let ua = info.user_agent.as_deref().unwrap_or(default_ua);
    if let Ok(v) = HeaderValue::from_str(ua) {
        h.insert(header::USER_AGENT, v);
    }
    h.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    // Byte ranges must refer to the identity encoding.
    h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    if let Some(r) = info.referrer.as_deref()
        && let Ok(v) = HeaderValue::from_str(r)
    {
        h.insert(header::REFERER, v);
    }
    if let Some(c) = info.cookies.as_deref()
        && let Ok(v) = HeaderValue::from_str(c)
    {
        h.insert(header::COOKIE, v);
    }
    for (k, v) in &info.headers {
        if is_managed_header(k) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            h.insert(name, value);
        }
    }
    h
}

/// Parses `Content-Range: bytes START-END/TOTAL` (TOTAL may be `*`).
/// Returns `(start, end_inclusive, total)`.
pub fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let v = v.trim();
    let rest = v
        .strip_prefix("bytes")
        .or_else(|| v.strip_prefix("Bytes"))?
        .trim_start_matches([' ', '=']);
    let (range, total) = rest.split_once('/')?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse::<u64>().ok()?),
    };
    let (s, e) = range.trim().split_once('-')?;
    let start = s.trim().parse::<u64>().ok()?;
    let end = e.trim().parse::<u64>().ok()?;
    if end < start || total.is_some_and(|t| end >= t) {
        return None;
    }
    Some((start, end, total))
}

/// Parses the total from an unsatisfied range: `bytes */1234`.
pub fn parse_unsatisfied_range(v: &str) -> Option<u64> {
    let rest = v.trim().strip_prefix("bytes")?.trim();
    let total = rest.strip_prefix("*/")?;
    total.trim().parse().ok()
}

/// Maps a non-success status to an error.
pub fn status_error(status: StatusCode) -> HttpError {
    let text = format!(
        "HTTP {} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
    .trim()
    .to_owned();
    let retry = status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS;
    if retry {
        HttpError::Retry(text)
    } else {
        let hint = match status.as_u16() {
            401 | 403 => " (the link may have expired or needs a login)",
            404 | 410 => " (file not found)",
            _ => "",
        };
        HttpError::Fatal(format!("{text}{hint}"))
    }
}

/// Human description of a reqwest error.
pub fn describe(e: &reqwest::Error) -> HttpError {
    if e.is_builder() {
        return HttpError::Fatal(format!("Invalid request: {e}"));
    }
    if e.is_redirect() {
        return HttpError::Fatal("Too many redirects".into());
    }
    let what = if e.is_timeout() {
        "Timed out".to_owned()
    } else if e.is_connect() {
        "Could not connect".to_owned()
    } else if e.is_body() || e.is_decode() {
        "Connection interrupted".to_owned()
    } else {
        "Network error".to_owned()
    };
    // Include the innermost cause, which is usually the useful bit.
    let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e);
    let mut last = None;
    while let Some(s) = src {
        last = Some(s.to_string());
        src = s.source();
    }
    HttpError::Retry(match last {
        Some(cause) => format!("{what}: {cause}"),
        None => what,
    })
}

fn header_string(h: &HeaderMap, name: header::HeaderName) -> Option<String> {
    h.get(name).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
}

/// Picks a name from the redirect chain: the final URL if it has an
/// extension, else the original URL if it has one, else whatever exists.
fn name_from_urls(final_url: &str, original: &str) -> Option<String> {
    let f = filename::name_from_url(final_url);
    let o = filename::name_from_url(original);
    let has_ext = |s: &Option<String>| s.as_deref().is_some_and(|n| filename::split_ext(n).1.is_some());
    if has_ext(&f) {
        f
    } else if has_ext(&o) {
        o
    } else {
        f.or(o)
    }
}

/// Probes a URL with `GET` + `Range: bytes=0-` (following redirects) and
/// reads only the headers: size, resumability, file name, content type.
pub async fn probe(
    client: &Client,
    url: &str,
    info: &RequestInfo,
    default_ua: &str,
    timeout: Duration,
) -> Result<ProbeInfo, HttpError> {
    let req = client
        .get(url)
        .headers(request_headers(info, default_ua))
        .header(header::RANGE, "bytes=0-");
    let resp = match tokio::time::timeout(timeout, req.send()).await {
        Err(_) => return Err(HttpError::Retry("Timed out waiting for the server".into())),
        Ok(Err(e)) => return Err(describe(&e)),
        Ok(Ok(r)) => r,
    };
    let status = resp.status();
    let headers = resp.headers().clone();
    let final_url = resp.url().to_string();
    drop(resp); // we only need the headers

    let content_length = header_string(&headers, header::CONTENT_LENGTH).and_then(|v| v.trim().parse::<u64>().ok());
    let (total_size, resumable) = if status == StatusCode::PARTIAL_CONTENT {
        match header_string(&headers, header::CONTENT_RANGE).and_then(|v| parse_content_range(&v)) {
            Some((0, _, Some(total))) => (Some(total), true),
            // Total unknown (`bytes 0-N/*`) or an odd range: stream it in one piece.
            _ => (None, false),
        }
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
        // Typically an empty file: `Content-Range: bytes */0`.
        match header_string(&headers, header::CONTENT_RANGE).and_then(|v| parse_unsatisfied_range(&v)) {
            Some(total) => (Some(total), false),
            None => return Err(status_error(status)),
        }
    } else if status.is_success() {
        (content_length, false)
    } else {
        return Err(status_error(status));
    };

    let mime = header_string(&headers, header::CONTENT_TYPE)
        .map(|m| m.split(';').next().unwrap_or("").trim().to_owned())
        .filter(|m| !m.is_empty());
    let cd_name = header_string(&headers, header::CONTENT_DISPOSITION)
        .and_then(|v| filename::parse_content_disposition(&v));
    let raw = cd_name
        .or_else(|| name_from_urls(&final_url, url))
        .unwrap_or_else(|| filename::DEFAULT_NAME.to_owned());
    let file_name = filename::ensure_extension(&filename::sanitize(&raw), mime.as_deref());

    Ok(ProbeInfo {
        final_url,
        total_size,
        resumable,
        file_name,
        mime,
    })
}

/// Exponential backoff: 1 s, 2 s, 4 s … capped at 30 s.
pub fn backoff(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.saturating_sub(1).min(5);
    Duration::from_secs(secs.min(30))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, 99, Some(1000))));
        assert_eq!(parse_content_range("bytes 100-199/*"), Some((100, 199, None)));
        assert_eq!(parse_content_range(" bytes  5-5/6 "), Some((5, 5, Some(6))));
        assert_eq!(parse_content_range("bytes=0-0/1"), Some((0, 0, Some(1))));
        assert_eq!(parse_content_range("bytes 0-1000/1000"), None); // end past total
        assert_eq!(parse_content_range("bytes 9-3/10"), None);
        assert_eq!(parse_content_range("bytes */1000"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
        assert_eq!(parse_content_range("garbage"), None);
        assert_eq!(parse_unsatisfied_range("bytes */0"), Some(0));
        assert_eq!(parse_unsatisfied_range("bytes */1234"), Some(1234));
        assert_eq!(parse_unsatisfied_range("bytes 0-1/2"), None);
    }

    #[test]
    fn status_classification() {
        assert!(matches!(status_error(StatusCode::NOT_FOUND), HttpError::Fatal(_)));
        assert!(matches!(status_error(StatusCode::FORBIDDEN), HttpError::Fatal(_)));
        assert!(matches!(status_error(StatusCode::SERVICE_UNAVAILABLE), HttpError::Retry(_)));
        assert!(matches!(status_error(StatusCode::TOO_MANY_REQUESTS), HttpError::Retry(_)));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(2));
        assert_eq!(backoff(4), Duration::from_secs(8));
        assert_eq!(backoff(6), Duration::from_secs(30));
        assert_eq!(backoff(40), Duration::from_secs(30));
    }

    #[test]
    fn browser_headers_are_forwarded() {
        let mut info = RequestInfo {
            referrer: Some("https://page.example/".into()),
            cookies: Some("a=b; c=d".into()),
            user_agent: Some("TestUA/1".into()),
            ..Default::default()
        };
        info.headers.insert("Authorization".into(), "Bearer x".into());
        info.headers.insert("Range".into(), "bytes=5-".into());
        let h = request_headers(&info, "Default/1");
        assert_eq!(h.get(header::USER_AGENT).unwrap(), "TestUA/1");
        assert_eq!(h.get(header::REFERER).unwrap(), "https://page.example/");
        assert_eq!(h.get(header::COOKIE).unwrap(), "a=b; c=d");
        assert_eq!(h.get(header::AUTHORIZATION).unwrap(), "Bearer x");
        assert!(h.get(header::RANGE).is_none());
        let h = request_headers(&RequestInfo::default(), "Default/1");
        assert_eq!(h.get(header::USER_AGENT).unwrap(), "Default/1");
    }
}
