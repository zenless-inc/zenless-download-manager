//! File-name helpers: `Content-Disposition` parsing (incl. RFC 5987
//! `filename*`), names derived from URLs, Windows-safe sanitising, MIME →
//! extension mapping and unique ("name (1).ext") paths.

use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};

/// Fallback when nothing better is known.
pub const DEFAULT_NAME: &str = "download";

/// Extracts the file name from a `Content-Disposition` header value.
/// `filename*` (RFC 5987/6266) wins over plain `filename`.
pub fn parse_content_disposition(value: &str) -> Option<String> {
    let mut plain: Option<String> = None;
    let mut extended: Option<String> = None;
    for param in split_params(value) {
        let Some((key, val)) = param.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        match key.as_str() {
            "filename*" => {
                if let Some(v) = decode_rfc5987(val) {
                    extended = Some(v);
                }
            }
            "filename" => plain = Some(decode_plain(&unquote(val))),
            _ => {}
        }
    }
    extended
        .or(plain)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Splits `attachment; filename="a;b.zip"; x=y` on `;` outside quotes.
fn split_params(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => {
                cur.push(ch);
                escaped = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                cur.push(ch);
            }
            ';' if !in_quotes => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        v.to_owned()
    }
}

/// Some servers percent-encode the plain `filename` too; decode it when it
/// is clearly encoded and decodes to valid UTF-8.
fn decode_plain(v: &str) -> String {
    let looks_encoded = v.as_bytes().windows(3).any(|w| {
        w[0] == b'%' && w[1].is_ascii_hexdigit() && w[2].is_ascii_hexdigit()
    });
    if looks_encoded
        && let Ok(s) = percent_decode_str(v).decode_utf8()
    {
        return s.into_owned();
    }
    v.to_owned()
}

/// Decodes `UTF-8''na%C3%AFve.txt` / `iso-8859-1'en'%A3.txt`.
fn decode_rfc5987(v: &str) -> Option<String> {
    let v = unquote(v);
    let mut parts = v.splitn(3, '\'');
    let charset = parts.next()?.trim().to_ascii_lowercase();
    let _lang = parts.next()?;
    let encoded = parts.next()?;
    let bytes: Vec<u8> = percent_decode_str(encoded).collect();
    let s = match charset.as_str() {
        "utf-8" | "utf8" | "" => String::from_utf8_lossy(&bytes).into_owned(),
        // ISO-8859-1 maps bytes 1:1 onto the first 256 code points.
        "iso-8859-1" | "latin1" | "us-ascii" => bytes.iter().map(|&b| b as char).collect(),
        _ => String::from_utf8_lossy(&bytes).into_owned(),
    };
    Some(s).filter(|s| !s.trim().is_empty())
}

/// Last non-empty path segment of a URL, percent-decoded (`None` for `/`).
pub fn name_from_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let seg = parsed
        .path_segments()?
        .rev()
        .find(|s| !s.is_empty())?
        .to_owned();
    let decoded = percent_decode_str(&seg).decode_utf8_lossy().into_owned();
    let decoded = decoded.trim().to_owned();
    (!decoded.is_empty()).then_some(decoded)
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes a name safe on Windows (and everywhere else): replaces reserved
/// characters, strips trailing dots/spaces, avoids device names like `CON`
/// and caps the length.
pub fn sanitize(name: &str) -> String {
    // Only keep the last path component if a server sent a path.
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut s: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 || c == '\u{7f}' => '_',
            c => c,
        })
        .collect();
    s = s.trim().trim_end_matches(['.', ' ']).trim_start().to_owned();
    if s.is_empty() || s.chars().all(|c| c == '.') {
        return DEFAULT_NAME.to_owned();
    }
    let stem = s.split('.').next().unwrap_or("").trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        s = format!("_{s}");
    }
    // Keep names well below MAX_PATH; preserve the extension.
    const MAX: usize = 180;
    if s.chars().count() > MAX {
        let (stem, ext) = split_ext(&s);
        let ext_len = ext.map_or(0, |e| e.chars().count() + 1);
        let keep: String = stem.chars().take(MAX.saturating_sub(ext_len)).collect();
        s = match ext {
            Some(e) => format!("{}.{e}", keep.trim_end()),
            None => keep,
        };
    }
    s
}

/// Splits `name.ext` into (`name`, `Some("ext")`). Leading-dot names have no extension.
pub fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty() && !ext.is_empty() && ext.len() <= 12 && !ext.contains(' ') =>
        {
            (stem, Some(ext))
        }
        _ => (name, None),
    }
}

/// Common MIME types → file extension (without dot).
pub fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let m = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match m.as_str() {
        "application/zip" | "application/x-zip-compressed" => "zip",
        "application/x-rar-compressed" | "application/vnd.rar" => "rar",
        "application/x-7z-compressed" => "7z",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/x-bzip2" => "bz2",
        "application/x-xz" => "xz",
        "application/zstd" => "zst",
        "application/x-iso9660-image" => "iso",
        "application/pdf" => "pdf",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.ms-powerpoint" => "ppt",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/epub+zip" => "epub",
        "application/rtf" => "rtf",
        "application/json" => "json",
        "application/xml" | "text/xml" => "xml",
        "application/x-msdownload" | "application/vnd.microsoft.portable-executable" => "exe",
        "application/x-msi" | "application/x-ms-installer" => "msi",
        "application/vnd.android.package-archive" => "apk",
        "application/x-apple-diskimage" => "dmg",
        "application/vnd.debian.binary-package" | "application/x-debian-package" => "deb",
        "application/x-rpm" => "rpm",
        "application/java-archive" => "jar",
        "application/x-bittorrent" => "torrent",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "text/html" => "html",
        "text/css" => "css",
        "text/javascript" | "application/javascript" => "js",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/avif" => "avif",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/aac" => "aac",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "video/x-msvideo" => "avi",
        "video/mpeg" => "mpg",
        "video/x-flv" => "flv",
        "video/mp2t" => "ts",
        _ => return None,
    })
}

/// Appends an extension derived from the MIME type when `name` has none.
pub fn ensure_extension(name: &str, mime: Option<&str>) -> String {
    if split_ext(name).1.is_some() {
        return name.to_owned();
    }
    match mime.and_then(extension_for_mime) {
        Some(ext) => format!("{name}.{ext}"),
        None => name.to_owned(),
    }
}

/// Picks the best file name for a response: Content-Disposition, then the
/// URL path, then [`DEFAULT_NAME`]; sanitised and with an extension from the
/// content type when missing.
pub fn pick_name(content_disposition: Option<&str>, url: &str, mime: Option<&str>) -> String {
    let raw = content_disposition
        .and_then(parse_content_disposition)
        .or_else(|| name_from_url(url))
        .unwrap_or_else(|| DEFAULT_NAME.to_owned());
    ensure_extension(&sanitize(&raw), mime)
}

/// Adds `" (1)"`, `" (2)"`, … before the extension until `taken` is false.
pub fn unique_name(name: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    let (stem, ext) = split_ext(name);
    for i in 1..10_000 {
        let candidate = match ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{stem} ({})", crate::util::unix_now())
}

/// Temp file used while downloading: `<name>.zdm` next to the final file.
pub fn temp_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(".zdm");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_plain() {
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="report 2024.pdf""#).as_deref(),
            Some("report 2024.pdf")
        );
        assert_eq!(
            parse_content_disposition("attachment; filename=plain.zip").as_deref(),
            Some("plain.zip")
        );
        assert_eq!(
            parse_content_disposition(r#"inline; filename="semi;colon \"q\".txt"; size=3"#).as_deref(),
            Some(r#"semi;colon "q".txt"#)
        );
        assert_eq!(parse_content_disposition("attachment"), None);
        assert_eq!(parse_content_disposition(r#"attachment; filename="""#), None);
    }

    #[test]
    fn content_disposition_rfc5987() {
        assert_eq!(
            parse_content_disposition(
                r#"attachment; filename="fallback.txt"; filename*=UTF-8''na%C3%AFve%20file.txt"#
            )
            .as_deref(),
            Some("naïve file.txt")
        );
        // filename* wins even when it comes first.
        assert_eq!(
            parse_content_disposition("attachment; filename*=utf-8'en'%E2%82%AC%20rates.pdf; filename=rates.pdf")
                .as_deref(),
            Some("€ rates.pdf")
        );
        assert_eq!(
            parse_content_disposition("attachment; filename*=iso-8859-1'en'%A3%20rates.txt").as_deref(),
            Some("£ rates.txt")
        );
        // Percent-encoded plain filename.
        assert_eq!(
            parse_content_disposition(r#"attachment; filename="%E4%B8%AD%E6%96%87.zip""#).as_deref(),
            Some("中文.zip")
        );
    }

    #[test]
    fn names_from_urls() {
        assert_eq!(
            name_from_url("https://example.com/files/My%20Setup.exe?token=1#x").as_deref(),
            Some("My Setup.exe")
        );
        assert_eq!(name_from_url("https://example.com/dir/").as_deref(), Some("dir"));
        assert_eq!(name_from_url("https://example.com/"), None);
        assert_eq!(name_from_url("not a url"), None);
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("a<b>c:d\"e|f?g*h.txt"), "a_b_c_d_e_f_g_h.txt");
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("C:\\Windows\\evil.dll"), "evil.dll");
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul.txt"), "_nul.txt");
        assert_eq!(sanitize("Com1.tar.gz"), "_Com1.tar.gz");
        assert_eq!(sanitize("console.log"), "console.log");
        assert_eq!(sanitize("trailing. . "), "trailing");
        assert_eq!(sanitize("   "), DEFAULT_NAME);
        assert_eq!(sanitize("..."), DEFAULT_NAME);
        assert_eq!(sanitize("tab\there"), "tab_here");
        let long = format!("{}.zip", "x".repeat(400));
        let s = sanitize(&long);
        assert!(s.ends_with(".zip") && s.chars().count() <= 180);
    }

    #[test]
    fn extensions_from_mime() {
        assert_eq!(ensure_extension("file", Some("application/zip")), "file.zip");
        assert_eq!(ensure_extension("file", Some("video/mp4; codecs=avc1")), "file.mp4");
        assert_eq!(ensure_extension("file.bin", Some("application/zip")), "file.bin");
        assert_eq!(ensure_extension("file", Some("application/octet-stream")), "file");
        assert_eq!(ensure_extension("file", None), "file");
    }

    #[test]
    fn picking_names() {
        assert_eq!(
            pick_name(Some("attachment; filename=\"a.zip\""), "https://x.y/b.bin", None),
            "a.zip"
        );
        assert_eq!(pick_name(None, "https://x.y/get", Some("application/pdf")), "get.pdf");
        assert_eq!(pick_name(None, "https://x.y/", None), DEFAULT_NAME);
    }

    #[test]
    fn unique_names() {
        let taken = ["a.zip", "a (1).zip", "noext"];
        assert_eq!(unique_name("b.zip", |n| taken.contains(&n)), "b.zip");
        assert_eq!(unique_name("a.zip", |n| taken.contains(&n)), "a (2).zip");
        assert_eq!(unique_name("noext", |n| taken.contains(&n)), "noext (1)");
    }

    #[test]
    fn temp_paths() {
        assert_eq!(temp_path(Path::new("C:/d/a.zip")), PathBuf::from("C:/d/a.zip.zdm"));
    }
}
