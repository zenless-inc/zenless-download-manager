# Zenless Download Manager

A fast, good-looking, IDM-style download manager for Windows, written in Rust with
[egui](https://github.com/emilk/egui). Part of the **Zenless** suite
([website](https://zenless-suite.vercel.app) · [downloads](https://zenless-suite.vercel.app/download)).

`zenless-dm.exe` · version 0.2.3 · MIT license

## Features

**Engine**

- **Multi-connection segmented downloads with IDM-style dynamic segmentation.** A download
  starts with *N* connections (default 8, 1–32). Whenever a connection finishes its segment, the
  largest remaining segment is split in half (if more than 1 MiB is left) and the free connection
  takes the second half, so fast connections keep working until the very end.
- **Fewer connections for small files.** Files up to 100 MB use 2 connections instead of the
  default, because many sites rate-limit or block clients that open lots of connections for one
  small file. It only applies when you haven't picked the number yourself, and the size and count
  are adjustable (or the rule can be turned off) in Settings → General.
- Positional writes into a pre-allocated `<name>.zdm` file in the target folder; renamed on
  completion. Existing files are never overwritten (`name (1).ext`).
- Smart probing (`GET` + `Range: bytes=0-`, redirects followed): size, resume support, file name
  from `Content-Disposition` (including RFC 5987 `filename*`) or the URL, Windows-safe names
  (reserved characters, `CON`/`NUL`/… device names), extension from the content type when missing.
- Servers without range support fall back to a single connection (the UI tells you that pausing
  restarts such a download).
- Pause, resume, restart, delete. Per-connection retries with exponential backoff (default 8 retries,
  1 s → 30 s) that never lose progress.
- Global speed limit (token bucket) and optional per-download limit.
- Queue with max. simultaneous downloads (default 3), FIFO order you can rearrange, start/stop queue.
- Everything persists (`downloads.json` incl. per-segment progress, `settings.json`; atomic,
  debounced writes). Downloads resume after a restart — automatically if you enable it.
- Browser context (referrer, cookies, user agent, extra headers) is sent with every request.
- Categories (Video, Music, Documents, Compressed, Programs, Images, Other) with optional
  category sub-folders.
- Smoothed speed + ETA, per-connection statistics, SHA-256 of finished files on demand.
- Optional clipboard monitor (off by default) that offers copied file links in a small toast.

**Interface**

- Toolbar (Add URL, Resume, Pause, Pause all, Delete, Start/Stop queue, Open folder, Settings)
  with search.
- Sidebar with status filters and categories (with counts) and a live speed summary.
- Sortable download table with category icons, progress bars, status pills, multi-select,
  context menu and a friendly empty state.
- Details panel: URL, folder, size, resume support, an IDM-style **segment map**, a
  per-connection list and a 60-second speed graph.
- "New download" dialog with a debounced live probe (size + resumable), folder picker, category
  and connection count; batch dialog with extension filters for many links at once.
- Settings: General, Network, Browser integration, Appearance (34 built-in themes shared by all
  Zenless apps + custom themes), About.
- Toasts for finished/failed downloads (and a taskbar flash when the window is in the background),
  drag & drop of links / `.url` files, single-instance with argument forwarding, `--minimized`,
  "Start with Windows".
- A **system tray icon** with live status; closing the window keeps the app running there
  (see [System tray](#system-tray)).
- **Automatic updates** of the app itself and of the browser extensions installed next to it
  (see [Updates](#updates)).

## Updates

The app updates itself from its [GitHub releases](https://github.com/zenless-inc/zenless-download-manager/releases):

- About 15 seconds after start (when the last check is more than 6 hours old) and then every
  6 hours, it asks `GET https://api.github.com/repos/zenless-inc/zenless-download-manager/releases/latest`.
  *Settings › About › Updates* shows the installed version, the status and when it last
  checked, and has **Check for updates** plus two switches: *Check for updates automatically*
  and *Download updates in the background and install them when I close the app* (both on by
  default).
- A newer release shows a slim banner under the toolbar: **Update now** · **What's new** ·
  **Later** (hide until the next start) · **Skip this version**. While it downloads, the
  banner shows progress; once it is ready: **Restart now** · **Later** (it installs
  automatically when you close the app).
- The download (`zenless-dm.exe` from the release) is only used when its size, its SHA-256
  (GitHub's asset `digest`, or the `zenless-dm.exe.sha256` file attached to the release) and
  its `MZ` header match. A release without a checksum is refused.
- Installing renames the running `zenless-dm.exe` to `zenless-dm.exe.old` (Windows allows
  that), copies the verified file into its place and either restarts right away or lets the
  next start run the new version; the next start deletes the `.old` file. "Restart now"
  passes the running downloads on, so they continue in the new version even with
  *Resume unfinished downloads* off, and the new version says "Updated to vX" with a
  *What's new* link. If the folder isn't writable, the banner offers *Download from website*.
- **Browser extensions:** when Zenless Setup's layout is next to the app
  (`..\Browser Extensions\Chrome\manifest.json` and/or `..\Browser Extensions\zenless-firefox-extension.xpi`),
  every update check also looks at the latest
  [Chrome](https://github.com/zenless-inc/zenless-chrome-extension/releases) and
  [Firefox](https://github.com/zenless-inc/zenless-firefox-extension/releases) extension releases.
  A newer, checksum-verified package replaces the unpacked `Chrome` folder (unpacked to
  `Chrome.new`, then swapped; files are overwritten in place if the folder is in use) or the
  `.xpi` (atomically), and a small toast says "Browser extension updated to vX". `GET /ping`
  reports the versions on disk in `extensions`.
- What the updater did is logged to `%APPDATA%\Zenless\DownloadManager\updates\updater.log`.

## System tray

While the app runs it shows an icon in the Windows notification area (Windows 11 may put new
icons in the overflow menu behind the `^` arrow; drag it onto the taskbar to keep it visible).

- **Tooltip** with the live status, refreshed every 2 seconds:
  `Zenless Download Manager · 2 active · 5.3 MB/s`, or `… · idle`.
- **Left click** shows the window (restored and focused) when it is hidden or minimized, and
  hides it when it is on screen.
- **Right-click menu:** *Show Zenless Download Manager* / *Hide*, *Add URL…* (opens the
  "New download" dialog), *Pause all*, *Resume all* (the paused downloads), *Quit*.
- **Closing the window** (title-bar X, Alt+F4) hides it to the tray while *Settings › General ›
  Keep running in the tray when the window is closed* is on (the default). Downloads go on;
  the first time, a Windows notification (and a note in the window once it's opened again)
  says that the app is still running. *Quit* in the tray menu exits.
- *Settings › General › Show an icon in the system tray* (on by default) removes the icon when
  turned off; the close button then quits again.
- `--minimized` (used by *Start with Windows*) starts hidden in the tray, or minimized to the
  taskbar without the tray icon.
- While the window is hidden, finished downloads are announced with a Windows notification
  (when *Show a notification when a download finishes* is on).
- Things that need the window bring it back from the tray: a link sent from the browser
  (`mode: ask`), `POST /focus`, starting `zenless-dm.exe` again. `POST /quit`, *Quit* and
  *Restart now* after an update always exit.

## Keyboard shortcuts

| Keys | Action |
|---|---|
| `Ctrl+N` | New download (pre-filled from the clipboard) |
| `Ctrl+V` | Paste one link (dialog) or many links (batch dialog) |
| `Ctrl+A` | Select all visible downloads |
| `Space` | Pause / resume the selection |
| `Delete` | Remove the selection |
| `Enter` | Open the finished file / show properties |
| `Esc` | Clear the selection / close a dialog |
| `Ctrl`/`Shift` + click | Multi-select |

## Command line

```
zenless-dm.exe [--minimized] [URL...]
```

The app is single-instance: a second launch forwards its URLs to the running instance
(`POST /download` for one URL, `POST /batch` for several), focuses it and exits.
`--minimized` starts hidden in the tray (or minimized when the tray icon is turned off).

After an update the previous version starts the new one with
`--updated-from <old version> [--resume <id,id,…>]`; it first waits (at most 20 s) until the
old instance stops answering `/ping`.

## Local API (browser extensions)

Plain HTTP/1.1 + JSON on **`127.0.0.1:6812`** only.

| Method | Path | Body / answer |
|---|---|---|
| `GET` | `/ping` | `{"ok":true,"app":"zenless-dm","name":"Zenless Download Manager","version":"0.2.3","extensions":{"chrome":"0.2.0","firefox":"0.2.0"}}` — `extensions` lists the versions installed next to the app (missing ones are left out) |
| `GET` | `/status` | counts, total speed and up to 8 recent unfinished items |
| `POST` | `/download` | `{"url", "filename"?, "referrer"?, "cookies"?, "user_agent"?, "headers"?, "size"?, "mime"?, "page_title"?, "source"?, "mode": "ask"｜"start"｜"queue"}` → `{"ok":true,"id"?}` |
| `POST` | `/batch` | `{"items":[{"url","filename"?}], "referrer"?, "cookies"?, "user_agent"?, "page_title"?, "source"?}` → batch dialog, `{"ok":true,"count":N}` |
| `POST` | `/focus` | restore and focus the window |
| `POST` | `/quit` | save and exit (only without an `Origin` header) |

Security rules:

1. Bound to `127.0.0.1` only; foreign `Host` headers are rejected (DNS-rebinding protection).
2. If an `Origin` header is present it must start with `chrome-extension://`, `moz-extension://`,
   `extension://` or `safari-web-extension://`, otherwise **403**.
3. Every `POST` needs an `X-Zenless-Client: <name/version>` header, otherwise **400**.
4. `OPTIONS` preflights get **204** with CORS headers for extension origins.
5. `/quit` and a local `save_dir` in `/download` are only accepted from callers without `Origin`.
6. JSON bodies up to 8 MiB; errors look like `{"ok":false,"error":"…"}`.

If port 6812 is taken the app keeps working and shows that browser integration is unavailable.

Example:

```sh
curl -s http://127.0.0.1:6812/ping
curl -s -X POST http://127.0.0.1:6812/download \
     -H "X-Zenless-Client: curl/1.0" -H "Content-Type: application/json" \
     -d '{"url":"https://proof.ovh.net/files/1Mb.dat","mode":"start"}'
```

## Files

| What | Where |
|---|---|
| Settings, download list | `%APPDATA%\Zenless\DownloadManager\settings.json`, `downloads.json` |
| Theme (shared by all Zenless apps) | `%APPDATA%\Zenless\appearance.json`, `themes\*.json` |
| Updates | `%APPDATA%\Zenless\DownloadManager\updater.json` (switches, skipped version, last check), `updates\` (downloads, `updater.log`) |
| Default save folder | your *Downloads* folder |
| Autostart | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` → `ZenlessDownloadManager` |

## Building

### Standard (MSVC)

Requires a recent stable Rust (edition 2024) and the Visual Studio C++ build tools.

```sh
cargo build --release
# → target\release\zenless-dm.exe
cargo test
```

The icon, version info and manifest are embedded by `build.rs` (via `rc.exe` / `embed-resource`).

### GNU toolchain + zig (no MSVC)

The project only uses pure-Rust dependencies (HTTPS via SChannel/`native-tls`), so it also builds
with the `x86_64-pc-windows-gnu` toolchain and zig as the C compiler / resource compiler:

```sh
rustup toolchain install stable-x86_64-pc-windows-gnu
export PATH="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-gnu/bin:$PATH"
export CC_x86_64_pc_windows_gnu="zig cc"
export AR_x86_64_pc_windows_gnu="zig ar"
cargo +stable-x86_64-pc-windows-gnu build --release
```

When `zig` is on `PATH`, `build.rs` uses `zig rc` to compile the Windows resources.

### Debug aids

| Variable | Effect |
|---|---|
| `ZENLESS_DEMO=1` | Fills the list with realistic, animated **fake** downloads. No network, no API server, nothing is saved. |
| `ZENLESS_DEMO_VIEW=…` | With demo mode: open `new`, `batch`, `properties`, `delete`, `settings`, `settings-network`, `settings-browser`, `settings-appearance`, `settings-about`, `settings-updates`, show the `empty` state, or a made-up update banner: `update-available`, `update-downloading`, `update-ready`, `update-failed`. |
| `ZENLESS_SCREENSHOT=out.png` | Saves a screenshot after `ZENLESS_SCREENSHOT_FRAMES` (default 40) frames and exits. |
| `ZENLESS_UPDATE_API=http://127.0.0.1:<port>` | Ask this server instead of `https://api.github.com` for releases (a local mock serving `/repos/<owner>/<repo>/releases/latest`). |
| `ZENLESS_UPDATE_DELAY_SECS=<n>` | First automatic update check after *n* seconds (instead of 15), even if the last check was recent. |
| `ZENLESS_UPDATE_TEST_RESTART=1` | Acts like a click on **Restart now** as soon as an update is ready (end-to-end tests). |
| `ZENLESS_DM_DATA_DIR=<dir>` | Use this folder instead of `%APPDATA%\Zenless\DownloadManager` (tests that must not touch real data). |
| `ZENLESS_DM_PORT=<port>` | Serve the local API on this port instead of 6812 (tests next to a running instance; the extensions only talk to 6812). |

Testing an update end to end: run an older build with `ZENLESS_UPDATE_API` pointing at a mock
that serves a release JSON (newer `tag_name`, an asset named `zenless-dm.exe` with a correct
`digest` of `sha256:<hex>`), `ZENLESS_UPDATE_DELAY_SECS=2` and a separate `ZENLESS_DM_DATA_DIR`.

## Project layout

```
src/
  lib.rs            core library (everything testable without a window)
  engine/           tokio engine: model, http probe, segments, limiter, worker, persistence
  api.rs            local HTTP API + single-instance client
  filename.rs       Content-Disposition, sanitising, unique names
  category.rs       extension → category
  extensions.rs     versions / replacement of the browser extensions next to the app
  settings.rs  clipboard.rs  autostart.rs  demo.rs  util.rs
  main.rs           entry point (CLI, single instance, window)
  ext_update.rs     refreshes the browser extensions on every update check
  ui/               egui front-end (toolbar, sidebar, table, details, dialogs, settings, toasts,
                    systray.rs: tray menu/tooltip and the close button)
  shared/           Zenless theme, UI kit, self-updater and tray icon (shared verbatim by all Zenless apps)
tests/engine_download.rs   end-to-end tests against a local HTTP server
```

## License

MIT — see [LICENSE](LICENSE). Copyright (c) 2026 Zenless.
