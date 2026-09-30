//! Small OS helpers: atomic file writes, opening files/folders, time.

use std::io;
use std::path::Path;

/// Current Unix time in seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Writes `bytes` to `path` atomically (temp file + rename), creating parent
/// directories as needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Opens a file (or folder) with the system's default application.
pub fn open_path(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `explorer <file>` opens it with the associated program.
        std::process::Command::new("explorer")
            .raw_arg(quote_windows(path))
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

/// Opens the containing folder and selects the file when possible.
pub fn reveal_in_folder(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if path.exists() {
            std::process::Command::new("explorer")
                .raw_arg(format!("/select,{}", quote_windows(path)))
                .spawn()
                .map(|_| ())
        } else {
            let dir = path.parent().unwrap_or(path);
            std::process::Command::new("explorer")
                .raw_arg(quote_windows(dir))
                .spawn()
                .map(|_| ())
        }
    }
    #[cfg(not(windows))]
    {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        open_path(dir)
    }
}

#[cfg(windows)]
fn quote_windows(path: &Path) -> String {
    // Explorer wants backslashes; paths cannot contain `"` on Windows.
    format!("\"{}\"", path.display().to_string().replace('/', "\\"))
}

/// Does `s` look like a single http(s) URL?
pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 8192 || s.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    reqwest::Url::parse(s).is_ok_and(|u| u.host_str().is_some())
}

/// Pulls every http(s) URL out of a blob of text (one per whitespace-separated token).
pub fn extract_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in text.split(|c: char| c.is_whitespace() || c == '"' || c == '<' || c == '>') {
        let t = token.trim_matches(|c: char| matches!(c, ',' | ';' | '\'' | '(' | ')' | '[' | ']'));
        // `.url` Internet shortcut files: `URL=https://...`
        let t = t.strip_prefix("URL=").unwrap_or(t);
        if looks_like_url(t) && !out.iter().any(|u| u == t) {
            out.push(t.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(looks_like_url("https://example.com/a.zip"));
        assert!(looks_like_url("  http://x.y/  "));
        assert!(!looks_like_url("ftp://x.y/a"));
        assert!(!looks_like_url("https://exa mple.com"));
        assert!(!looks_like_url("hello"));
    }

    #[test]
    fn url_extraction() {
        let text = "[InternetShortcut]\r\nURL=https://a.b/c.zip\r\nsee (https://d.e/f.iso), https://a.b/c.zip";
        assert_eq!(extract_urls(text), vec!["https://a.b/c.zip", "https://d.e/f.iso"]);
    }

    #[test]
    fn atomic_write() {
        let dir = std::env::temp_dir().join(format!("zdm-atomic-{}", std::process::id()));
        let p = dir.join("x.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(dir);
    }
}
