# Zenless Download Manager

A fast, good-looking, IDM-style download manager for Windows, written in Rust with
[egui](https://github.com/emilk/egui). Part of the **Zenless** suite
([website](https://zenless-suite.vercel.app) · [downloads](https://zenless-suite.vercel.app/download)).

`zenless-dm.exe` · version 0.1.0 · MIT license

## Features

**Engine**

- **Multi-connection segmented downloads with IDM-style dynamic segmentation.** A download
  starts with *N* connections (default 8, 1–32). Whenever a connection finishes its segment, the
  largest remaining segment is split in half (if more than 1 MiB is left) and the free connection
  takes the second half, so fast connections keep working until the very end.
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
- Settings: General, Network, Browser integration, Appearance (13 built-in themes shared by all
  Zenless apps + custom themes), About.
- Toasts for finished/failed downloads (and a taskbar flash when the window is in the background),
  drag & drop of links / `.url` files, single-instance with argument forwarding, `--minimized`,
  "Start with Windows".

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

## Local API (browser extensions)

Plain HTTP/1.1 + JSON on **`127.0.0.1:6812`** only.

| Method | Path | Body / answer |
|---|---|---|
| `GET` | `/ping` | `{"ok":true,"app":"zenless-dm","name":"Zenless Download Manager","version":"0.1.0"}` |
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
| `ZENLESS_DEMO_VIEW=…` | With demo mode: open `new`, `batch`, `properties`, `delete`, `settings`, `settings-network`, `settings-browser`, `settings-appearance`, `settings-about`, or show the `empty` state. |
| `ZENLESS_SCREENSHOT=out.png` | Saves a screenshot after `ZENLESS_SCREENSHOT_FRAMES` (default 40) frames and exits. |

## Project layout

```
src/
  lib.rs            core library (everything testable without a window)
  engine/           tokio engine: model, http probe, segments, limiter, worker, persistence
  api.rs            local HTTP API + single-instance client
  filename.rs       Content-Disposition, sanitising, unique names
  category.rs       extension → category
  settings.rs  clipboard.rs  autostart.rs  demo.rs  util.rs
  main.rs           entry point (CLI, single instance, window)
  ui/               egui front-end (toolbar, sidebar, table, details, dialogs, settings, toasts)
  shared/           Zenless theme + UI kit (shared verbatim by all Zenless apps)
tests/engine_download.rs   end-to-end tests against a local HTTP server
```

## License

MIT — see [LICENSE](LICENSE). Copyright (c) 2026 Zenless.
