//! Keeps the browser extensions that Zenless Setup installed next to the app
//! (`<exe dir>\..\Browser Extensions\`) up to date: on every update check
//! (see [`crate::shared::updater::Updater::on_check`]) the latest releases of
//! the Chrome and Firefox extensions are compared with what is on disk, and a
//! newer, checksum-verified package replaces the unpacked `Chrome` folder or
//! the `.xpi`. This runs silently on the updater thread: it logs to
//! `updater.log` and leaves a short notice the UI shows as a toast.

use crate::shared::updater::{self, CheckContext};
use std::sync::{Arc, Mutex};
use zenless_dm::extensions::{self, Layout};

/// `(title, body)` toasts for the UI, drained in `App::logic`.
pub type Notices = Arc<Mutex<Vec<(String, String)>>>;

/// First bytes of a zip / xpi.
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";

#[derive(Clone, Copy, Debug)]
enum Kind {
    Chrome,
    Firefox,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Chrome => "Chrome",
            Kind::Firefox => "Firefox",
        }
    }

    fn repo(self) -> &'static str {
        match self {
            Kind::Chrome => extensions::CHROME_REPO,
            Kind::Firefox => extensions::FIREFOX_REPO,
        }
    }

    fn asset(self) -> &'static str {
        match self {
            Kind::Chrome => extensions::CHROME_ASSET,
            Kind::Firefox => extensions::FIREFOX_ASSET,
        }
    }
}

/// The updater hook. Does nothing without the installer layout.
pub fn hook(notices: Notices) -> impl FnMut(&CheckContext<'_>) + Send + 'static {
    move |cx: &CheckContext<'_>| {
        let Some(layout) = extensions::installed_layout() else { return };
        extensions::repair_chrome_dir(&layout.chrome_dir);
        for kind in [Kind::Chrome, Kind::Firefox] {
            if cx.cancelled() {
                return;
            }
            match refresh(cx, &layout, kind) {
                Ok(Some(version)) => {
                    cx.log(&format!("{} extension updated to {version}", kind.name()));
                    notices
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((format!("Browser extension updated to v{version}"), kind.name().to_owned()));
                    cx.request_repaint();
                }
                Ok(None) => {}
                Err(e) => cx.log(&format!("{} extension: {e}", kind.name())),
            }
        }
    }
}

/// Updates one extension if its release is newer. `Ok(Some(version))` when
/// it was replaced.
fn refresh(cx: &CheckContext<'_>, layout: &Layout, kind: Kind) -> Result<Option<String>, String> {
    let installed = match kind {
        Kind::Chrome if layout.has_chrome() => extensions::chrome_version(&layout.chrome_dir),
        Kind::Firefox if layout.has_firefox() => extensions::package_version(&layout.xpi).ok(),
        _ => return Ok(None),
    }
    // An unreadable manifest gets repaired by any release.
    .unwrap_or_else(|| "0.0.0".to_owned());
    let user_agent = cx.config.user_agent();
    let release = updater::fetch_latest(cx.api, kind.repo(), kind.asset(), &user_agent)?;
    if !updater::is_newer(&release.version, &installed) {
        return Ok(None);
    }
    if !release.has_asset() {
        return Err(format!("release {} has no {}", release.tag, kind.asset()));
    }
    if cx.cancelled() {
        return Ok(None);
    }
    let package = cx.config.updates_dir().join(format!("{}-{}", release.version, kind.asset()));
    updater::download_verified(
        &release.asset_url,
        &user_agent,
        release.asset_size,
        release.sha256.as_deref(),
        ZIP_MAGIC,
        &package,
        &mut |_, _| !cx.cancelled(),
    )?;
    let result = match kind {
        Kind::Chrome => extensions::install_chrome_package(&package, &layout.chrome_dir, &release.version),
        Kind::Firefox => extensions::install_xpi(&package, &layout.xpi, &release.version),
    };
    let _ = std::fs::remove_file(&package);
    result.map(|()| Some(release.version))
}
