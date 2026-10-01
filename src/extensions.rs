//! The browser extensions Zenless Setup installs next to the apps:
//! `<root>\Browser Extensions\Chrome\` (the unpacked Chrome/Chromium
//! extension, loaded with "Load unpacked") and
//! `<root>\Browser Extensions\zenless-firefox-extension.xpi`, where `<root>`
//! is the folder above `Download Manager\zenless-dm.exe`.
//!
//! This module reads their versions (`GET /ping` reports them) and swaps in a
//! newer release package. Finding and downloading releases is the updater's
//! job (`src/ext_update.rs` in the app).

use serde_json::{Map, Value};
use std::fs;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

pub const FOLDER: &str = "Browser Extensions";
pub const CHROME_DIR: &str = "Chrome";
pub const XPI_NAME: &str = "zenless-firefox-extension.xpi";
pub const CHROME_REPO: &str = "zenless-inc/zenless-chrome-extension";
pub const CHROME_ASSET: &str = "zenless-chrome-extension.zip";
pub const FIREFOX_REPO: &str = "zenless-inc/zenless-firefox-extension";
pub const FIREFOX_ASSET: &str = "zenless-firefox-extension.xpi";

/// Largest package accepted when unpacking (the real ones are < 1 MB).
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;

/// Where the extensions live for a given app executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// `<root>\Browser Extensions`.
    pub root: PathBuf,
    /// `<root>\Browser Extensions\Chrome`.
    pub chrome_dir: PathBuf,
    /// `<root>\Browser Extensions\zenless-firefox-extension.xpi`.
    pub xpi: PathBuf,
}

impl Layout {
    /// `<exe dir>\..\Browser Extensions`.
    pub fn for_exe(exe: &Path) -> Option<Self> {
        let root = exe.parent()?.parent()?.join(FOLDER);
        Some(Self { chrome_dir: root.join(CHROME_DIR), xpi: root.join(XPI_NAME), root })
    }

    pub fn has_chrome(&self) -> bool {
        self.chrome_dir.join("manifest.json").is_file()
    }

    pub fn has_firefox(&self) -> bool {
        self.xpi.is_file()
    }
}

/// The installer layout next to the running exe, if there is one.
pub fn installed_layout() -> Option<Layout> {
    Layout::for_exe(&std::env::current_exe().ok()?).filter(|l| l.root.is_dir())
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// The `version` of a `manifest.json`.
pub fn manifest_version(json: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(strip_bom(json)).ok()?;
    let version = v.get("version")?.as_str()?.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// Version of the unpacked extension in `dir`.
pub fn chrome_version(dir: &Path) -> Option<String> {
    manifest_version(&fs::read(dir.join("manifest.json")).ok()?)
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<fs::File>, String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    zip::ZipArchive::new(file).map_err(|e| format!("{} isn't a valid package ({e})", path.display()))
}

/// `""` when `manifest.json` is at the root of the package, `"<dir>/"` when
/// everything sits in a single top-level folder.
fn package_prefix<R: Read + Seek>(zip: &zip::ZipArchive<R>) -> Result<String, String> {
    let names: Vec<&str> = zip.file_names().collect();
    if names.contains(&"manifest.json") {
        return Ok(String::new());
    }
    let nested: Vec<&str> =
        names.iter().copied().filter(|n| n.ends_with("/manifest.json") && n.matches('/').count() == 1).collect();
    if let [one] = nested.as_slice() {
        let prefix = &one[..one.len() - "manifest.json".len()];
        if names.iter().all(|n| n.starts_with(prefix)) {
            return Ok(prefix.to_owned());
        }
    }
    Err("The package has no manifest.json".to_owned())
}

/// The `manifest.json` version inside a `.zip` / `.xpi` package.
pub fn package_version(path: &Path) -> Result<String, String> {
    let mut zip = open_zip(path)?;
    let prefix = package_prefix(&zip)?;
    let mut entry = zip
        .by_name(&format!("{prefix}manifest.json"))
        .map_err(|e| format!("Couldn't read manifest.json ({e})"))?;
    let mut bytes = Vec::new();
    entry
        .by_ref()
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Couldn't read manifest.json ({e})"))?;
    manifest_version(&bytes).ok_or_else(|| "manifest.json has no version".to_owned())
}

/// Unpacks a package into `dest` (created), dropping a single top-level
/// folder if the package has one. Refuses paths that would leave `dest`.
pub fn extract_package(package: &Path, dest: &Path) -> Result<(), String> {
    let mut zip = open_zip(package)?;
    let prefix = package_prefix(&zip)?;
    let prefix = Path::new(prefix.trim_end_matches('/'));
    fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("Broken package ({e})"))?;
        let Some(name) = entry.enclosed_name() else {
            return Err(format!("The package contains an unsafe path ({})", entry.name()));
        };
        let Ok(rel) = name.strip_prefix(prefix) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            continue;
        }
        if let Some(dir) = out.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut file = fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let budget = MAX_UNPACKED.saturating_sub(total) + 1;
        total += io::copy(&mut entry.by_ref().take(budget), &mut file).map_err(|e| format!("{}: {e}", out.display()))?;
        if total > MAX_UNPACKED {
            return Err("The package is too large".to_owned());
        }
    }
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn same_version(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_start_matches(['v', 'V']).to_owned();
    norm(a) == norm(b)
}

/// Copies `src` over `dst` file by file and deletes whatever `src` doesn't
/// have, so `dst` ends up identical. Used when `dst` can't be renamed because
/// something holds a file in it open.
pub fn sync_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            if to.is_file() {
                fs::remove_file(&to)?;
            }
            sync_dir(&entry.path(), &to)?;
        } else {
            if to.is_dir() {
                fs::remove_dir_all(&to)?;
            }
            // Plain open + truncate: works on files others have open for reading.
            io::copy(&mut fs::File::open(entry.path())?, &mut fs::File::create(&to)?)?;
        }
    }
    for entry in fs::read_dir(dst)? {
        let entry = entry?;
        if !src.join(entry.file_name()).exists() {
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// Finishes or undoes a folder swap that was interrupted (the app was killed
/// between the two renames of [`install_chrome_package`]).
pub fn repair_chrome_dir(chrome_dir: &Path) {
    let old = with_suffix(chrome_dir, ".old");
    let new = with_suffix(chrome_dir, ".new");
    if !chrome_dir.exists() && old.join("manifest.json").is_file() {
        let _ = fs::rename(&old, chrome_dir);
    }
    if chrome_dir.join("manifest.json").is_file() {
        for leftover in [old, new] {
            if leftover.exists() {
                let _ = fs::remove_dir_all(&leftover);
            }
        }
    }
}

/// Replaces the unpacked Chrome extension in `chrome_dir` with `package`
/// (whose manifest must say `expected_version`): unpack to `Chrome.new`,
/// rename `Chrome` → `Chrome.old` and `Chrome.new` → `Chrome`, delete the old
/// folder. If the folder can't be renamed (a file in it is in use), the files
/// are overwritten in place instead.
pub fn install_chrome_package(package: &Path, chrome_dir: &Path, expected_version: &str) -> Result<(), String> {
    let new_dir = with_suffix(chrome_dir, ".new");
    let old_dir = with_suffix(chrome_dir, ".old");
    if new_dir.exists() {
        fs::remove_dir_all(&new_dir).map_err(|e| format!("{}: {e}", new_dir.display()))?;
    }
    if let Err(e) = extract_package(package, &new_dir) {
        let _ = fs::remove_dir_all(&new_dir);
        return Err(e);
    }
    match chrome_version(&new_dir) {
        Some(v) if same_version(&v, expected_version) => {}
        other => {
            let _ = fs::remove_dir_all(&new_dir);
            return Err(format!(
                "The package's manifest says {}, the release says {expected_version}",
                other.as_deref().unwrap_or("no version")
            ));
        }
    }
    if old_dir.exists() {
        let _ = fs::remove_dir_all(&old_dir);
    }
    let moved_away = !chrome_dir.exists() || fs::rename(chrome_dir, &old_dir).is_ok();
    if moved_away {
        if fs::rename(&new_dir, chrome_dir).is_ok() {
            let _ = fs::remove_dir_all(&old_dir);
            return Ok(());
        }
        if old_dir.exists() {
            fs::rename(&old_dir, chrome_dir).map_err(|e| format!("Couldn't restore {}: {e}", chrome_dir.display()))?;
        }
    }
    let result = sync_dir(&new_dir, chrome_dir);
    let _ = fs::remove_dir_all(&new_dir);
    result.map_err(|e| format!("Couldn't update {}: {e}", chrome_dir.display()))
}

/// Replaces the Firefox package atomically (copy next to it, then rename
/// over it). The package's manifest must say `expected_version`.
pub fn install_xpi(package: &Path, xpi: &Path, expected_version: &str) -> Result<(), String> {
    let version = package_version(package)?;
    if !same_version(&version, expected_version) {
        return Err(format!("The package's manifest says {version}, the release says {expected_version}"));
    }
    let tmp = with_suffix(xpi, ".new");
    fs::copy(package, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, xpi).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Couldn't replace {}: {e}", xpi.display())
    })
}

type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

struct Cached {
    root: PathBuf,
    chrome: Stamp,
    firefox: Stamp,
    versions: Map<String, Value>,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// `{"chrome": "<version>", "firefox": "<version>"}` for a layout; keys of
/// missing extensions are left out. Cached until the files change.
pub fn versions_for(layout: &Layout) -> Map<String, Value> {
    let chrome = stamp(&layout.chrome_dir.join("manifest.json"));
    let firefox = stamp(&layout.xpi);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.as_ref()
        && c.root == layout.root
        && c.chrome == chrome
        && c.firefox == firefox
    {
        return c.versions.clone();
    }
    let mut versions = Map::new();
    if chrome.is_some()
        && let Some(v) = chrome_version(&layout.chrome_dir)
    {
        versions.insert("chrome".into(), v.into());
    }
    if firefox.is_some()
        && let Ok(v) = package_version(&layout.xpi)
    {
        versions.insert("firefox".into(), v.into());
    }
    *cache = Some(Cached { root: layout.root.clone(), chrome, firefox, versions: versions.clone() });
    versions
}

/// The versions installed next to the running exe (for `GET /ping`).
pub fn installed_versions() -> Map<String, Value> {
    installed_layout().map(|l| versions_for(&l)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zdm-ext-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(version: &str) -> String {
        format!(r#"{{"manifest_version":3,"name":"Zenless Browser Integration","version":"{version}"}}"#)
    }

    /// Writes a zip with `(name, contents)` entries.
    fn make_zip(path: &Path, entries: &[(&str, &str)]) {
        let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }

    fn chrome_package(path: &Path, version: &str) {
        make_zip(
            path,
            &[
                ("manifest.json", &manifest(version)),
                ("src/background.js", &format!("// v{version}")),
                ("icons/icon-16.png", "png"),
            ],
        );
    }

    #[test]
    fn layout_and_manifest_versions() {
        let l = Layout::for_exe(Path::new(r"C:\Zenless\Download Manager\zenless-dm.exe")).unwrap();
        assert_eq!(l.root, Path::new(r"C:\Zenless\Browser Extensions"));
        assert_eq!(l.chrome_dir, Path::new(r"C:\Zenless\Browser Extensions\Chrome"));
        assert_eq!(l.xpi, Path::new(r"C:\Zenless\Browser Extensions\zenless-firefox-extension.xpi"));
        assert_eq!(manifest_version(manifest("0.1.0").as_bytes()).as_deref(), Some("0.1.0"));
        assert_eq!(manifest_version(format!("\u{feff}{}", manifest("1.2.3")).as_bytes()).as_deref(), Some("1.2.3"));
        assert_eq!(manifest_version(br#"{"name":"x"}"#), None);
        assert_eq!(manifest_version(b"not json"), None);
    }

    #[test]
    fn package_versions_and_safe_extraction() {
        let dir = temp_dir("pkg");
        let root = dir.join("root.zip");
        chrome_package(&root, "0.1.1");
        assert_eq!(package_version(&root).unwrap(), "0.1.1");
        let nested = dir.join("nested.zip");
        make_zip(&nested, &[("ext/manifest.json", &manifest("0.2.0")), ("ext/src/a.js", "a")]);
        assert_eq!(package_version(&nested).unwrap(), "0.2.0");
        extract_package(&nested, &dir.join("out")).unwrap();
        assert!(dir.join("out/manifest.json").is_file() && dir.join("out/src/a.js").is_file(), "top folder dropped");

        let evil = dir.join("evil.zip");
        make_zip(&evil, &[("manifest.json", &manifest("0.1.1")), ("../escaped.txt", "x")]);
        let err = extract_package(&evil, &dir.join("evil-out")).unwrap_err();
        assert!(err.contains("unsafe"), "{err}");
        assert!(!dir.join("escaped.txt").exists());

        let empty = dir.join("empty.zip");
        make_zip(&empty, &[("readme.txt", "no manifest")]);
        assert!(package_version(&empty).is_err());
        fs::write(dir.join("junk.zip"), b"not a zip").unwrap();
        assert!(package_version(&dir.join("junk.zip")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn chrome_folder_swap() {
        let dir = temp_dir("swap");
        let chrome = dir.join("Chrome");
        fs::create_dir_all(chrome.join("src")).unwrap();
        fs::write(chrome.join("manifest.json"), manifest("0.1.0")).unwrap();
        fs::write(chrome.join("src/old-only.js"), "gone after the update").unwrap();
        let pkg = dir.join("pkg.zip");
        chrome_package(&pkg, "0.1.1");

        let err = install_chrome_package(&pkg, &chrome, "0.1.2").unwrap_err();
        assert!(err.contains("0.1.1"), "{err}");
        assert_eq!(chrome_version(&chrome).as_deref(), Some("0.1.0"), "untouched after a mismatch");

        install_chrome_package(&pkg, &chrome, "v0.1.1").unwrap();
        assert_eq!(chrome_version(&chrome).as_deref(), Some("0.1.1"));
        assert_eq!(fs::read_to_string(chrome.join("src/background.js")).unwrap(), "// v0.1.1");
        assert!(!chrome.join("src/old-only.js").exists());
        assert!(!dir.join("Chrome.new").exists() && !dir.join("Chrome.old").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_place_sync_and_repair() {
        let dir = temp_dir("sync");
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("a/b/c.txt"), "new").unwrap();
        fs::write(src.join("top.txt"), "top").unwrap();
        fs::create_dir_all(dst.join("a/stale-dir")).unwrap();
        fs::write(dst.join("a/stale-dir/x.txt"), "x").unwrap();
        fs::write(dst.join("stale.txt"), "x").unwrap();
        fs::write(dst.join("top.txt"), "old top, longer than the new one").unwrap();
        sync_dir(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("a/b/c.txt")).unwrap(), "new");
        assert_eq!(fs::read_to_string(dst.join("top.txt")).unwrap(), "top");
        assert!(!dst.join("stale.txt").exists() && !dst.join("a/stale-dir").exists());

        // Interrupted swap: only Chrome.old is left → it becomes Chrome again.
        let chrome = dir.join("Chrome");
        fs::create_dir_all(dir.join("Chrome.old")).unwrap();
        fs::write(dir.join("Chrome.old/manifest.json"), manifest("0.1.0")).unwrap();
        fs::create_dir_all(dir.join("Chrome.new")).unwrap();
        repair_chrome_dir(&chrome);
        assert_eq!(chrome_version(&chrome).as_deref(), Some("0.1.0"));
        assert!(!dir.join("Chrome.old").exists() && !dir.join("Chrome.new").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// A file held open without delete sharing blocks renaming the folder;
    /// the update then overwrites the files in place.
    #[cfg(windows)]
    #[test]
    fn chrome_update_with_a_file_in_use() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("inuse");
        let chrome = dir.join("Chrome");
        fs::create_dir_all(chrome.join("src")).unwrap();
        fs::write(chrome.join("manifest.json"), manifest("0.1.0")).unwrap();
        fs::write(chrome.join("src/background.js"), "// old").unwrap();
        fs::write(chrome.join("stale.js"), "x").unwrap();
        let pkg = dir.join("pkg.zip");
        chrome_package(&pkg, "0.1.1");
        const FILE_SHARE_READ: u32 = 1;
        const FILE_SHARE_WRITE: u32 = 2;
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(chrome.join("src/background.js"))
            .unwrap();
        assert!(fs::rename(&chrome, dir.join("probe")).is_err(), "the open file blocks renaming the folder");
        install_chrome_package(&pkg, &chrome, "0.1.1").unwrap();
        drop(held);
        assert_eq!(chrome_version(&chrome).as_deref(), Some("0.1.1"));
        assert_eq!(fs::read_to_string(chrome.join("src/background.js")).unwrap(), "// v0.1.1");
        assert!(!chrome.join("stale.js").exists());
        assert!(!dir.join("Chrome.new").exists() && !dir.join("Chrome.old").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn xpi_replacement_and_versions() {
        let dir = temp_dir("xpi");
        let exe = dir.join("Download Manager").join("zenless-dm.exe");
        let layout = Layout::for_exe(&exe).unwrap();
        assert!(versions_for(&layout).is_empty());
        fs::create_dir_all(&layout.chrome_dir).unwrap();
        fs::write(layout.chrome_dir.join("manifest.json"), manifest("0.1.0")).unwrap();
        make_zip(&layout.xpi, &[("manifest.json", &manifest("0.1.0"))]);
        let v = versions_for(&layout);
        assert_eq!(v.get("chrome").and_then(Value::as_str), Some("0.1.0"));
        assert_eq!(v.get("firefox").and_then(Value::as_str), Some("0.1.0"));

        let pkg = dir.join("new.xpi");
        make_zip(&pkg, &[("manifest.json", &manifest("0.1.1")), ("src/background.js", "//")]);
        assert!(install_xpi(&pkg, &layout.xpi, "0.1.2").is_err());
        install_xpi(&pkg, &layout.xpi, "0.1.1").unwrap();
        assert_eq!(package_version(&layout.xpi).unwrap(), "0.1.1");
        assert!(!with_suffix(&layout.xpi, ".new").exists());
        assert_eq!(versions_for(&layout).get("firefox").and_then(Value::as_str), Some("0.1.1"));
        fs::remove_dir_all(&layout.chrome_dir).unwrap();
        let v = versions_for(&layout);
        assert!(!v.contains_key("chrome"), "missing extensions are left out: {v:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
