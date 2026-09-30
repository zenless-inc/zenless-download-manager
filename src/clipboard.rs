//! Optional clipboard monitor: offers to download copied URLs that point at
//! files (by extension). Off by default; polls once per second while enabled.

use crate::category::{extension, is_downloadable_extension};
use crate::engine::{Notifier, UiEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Does this copied text look like a downloadable file URL?
pub fn is_downloadable_url(text: &str) -> bool {
    let t = text.trim();
    if !crate::util::looks_like_url(t) {
        return false;
    }
    crate::filename::name_from_url(t)
        .and_then(|n| extension(&n))
        .is_some_and(|e| is_downloadable_extension(&e))
}

/// Starts the polling thread. `enabled` can be toggled at any time.
pub fn spawn_monitor(enabled: Arc<AtomicBool>, events: std::sync::mpsc::Sender<UiEvent>, notify: Notifier) {
    let _ = std::thread::Builder::new()
        .name("zdm-clipboard".into())
        .spawn(move || {
            let mut clipboard: Option<arboard::Clipboard> = None;
            let mut last: Option<String> = None;
            let mut was_enabled = false;
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                if !enabled.load(Ordering::Relaxed) {
                    was_enabled = false;
                    clipboard = None;
                    continue;
                }
                if clipboard.is_none() {
                    clipboard = arboard::Clipboard::new().ok();
                }
                let Some(cb) = clipboard.as_mut() else { continue };
                let text = cb.get_text().ok().map(|t| t.trim().to_owned());
                if !was_enabled {
                    // Don't offer whatever was already on the clipboard.
                    was_enabled = true;
                    last = text;
                    continue;
                }
                if text.is_some() && text != last {
                    last = text.clone();
                    if let Some(t) = text
                        && is_downloadable_url(&t)
                    {
                        let _ = events.send(UiEvent::Clipboard(t));
                        notify();
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloadable_urls() {
        assert!(is_downloadable_url("https://example.com/files/setup.exe"));
        assert!(is_downloadable_url("https://example.com/a/b/movie.mkv?sig=abc"));
        assert!(is_downloadable_url("  http://x.y/archive.tar.gz  "));
        assert!(!is_downloadable_url("https://example.com/"));
        assert!(!is_downloadable_url("https://example.com/page.html"));
        assert!(!is_downloadable_url("https://example.com/article"));
        assert!(!is_downloadable_url("just some text.zip"));
    }
}
