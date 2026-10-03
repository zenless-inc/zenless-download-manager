//! System tray (notification area) icon for the Zenless desktop apps.
//!
//! This file is shared verbatim between Zenless Download Manager and Zenless
//! Torrent. Keep the copies in sync; app-specific menus, tooltips and actions
//! live in each app's own modules.
//!
//! * [`Tray`] shows the icon with a tooltip and a right-click menu built from
//!   [`MenuEntry`]s and reports [`TrayEvent`]s. Windows only: elsewhere
//!   [`Tray::new`] returns an error and the app runs without a tray icon (the
//!   module still compiles, so `cargo check` stays portable).
//! * [`WindowState`] shows, hides and toggles the main window. egui reports
//!   "minimized" but not "hidden", so hiding goes through it.
//! * [`close_action`] decides what closing the window does.
//! * [`speed`] formats speeds compactly for tooltips.
//!
//! How events reach the app: the shell talks to a hidden window that
//! `tray-icon` creates on the creating (main) thread, so clicks and menu
//! choices arrive inside eframe's message loop, also while the app window is
//! hidden. The handlers installed here put them into a channel and call
//! `request_repaint()`, which makes eframe run `App::logic()` (the only hook
//! that runs while the window is hidden or minimized); the app drains them
//! there with [`Tray::poll`].
//!
//! The icon disappears when the [`Tray`] is dropped. Drop it in `on_exit` so
//! no dead icon is left behind until the mouse passes over it.

use eframe::egui;

/// An entry of the tray icon's right-click menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    /// A clickable item; `id` comes back in [`TrayEvent::Menu`].
    Item { id: &'static str, label: String },
    Separator,
}

impl MenuEntry {
    pub fn item(id: &'static str, label: impl Into<String>) -> Self {
        Self::Item { id, label: label.into() }
    }
}

/// Something the user did with the tray icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left click on the icon (a double click counts once).
    Activate,
    /// A menu item was chosen; the id of its [`MenuEntry::Item`].
    Menu(String),
}

/// Whether this platform has a tray icon (for enabling the settings).
pub fn supported() -> bool {
    cfg!(windows)
}

/// What closing the main window (title-bar X, Alt+F4, the taskbar's "Close
/// window") should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Let eframe close the window and exit the app.
    Exit,
    /// Cancel the close and hide the window; the app keeps running in the tray.
    HideToTray,
}

/// The close decision. `quitting`: the app itself is exiting (tray "Quit",
/// the API's `/quit`, restart for an update…), which always wins.
/// `tray_icon`: the icon exists; without it a hidden window couldn't be
/// brought back. `close_to_tray`: the user's setting.
pub fn close_action(quitting: bool, tray_icon: bool, close_to_tray: bool) -> CloseAction {
    if !quitting && tray_icon && close_to_tray {
        CloseAction::HideToTray
    } else {
        CloseAction::Exit
    }
}

/// Shows, hides and toggles the main window and remembers whether it is
/// hidden (egui's `ViewportInfo` knows "minimized", not "hidden").
#[derive(Debug, Default)]
pub struct WindowState {
    hidden: bool,
}

impl WindowState {
    /// Hidden in the tray (`ViewportCommand::Visible(false)`).
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Neither hidden nor minimized.
    pub fn on_screen(&self, ctx: &egui::Context) -> bool {
        !self.hidden && ctx.input(|i| i.viewport().minimized) != Some(true)
    }

    /// Shows, restores and focuses the window.
    pub fn show(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.hidden = false;
        ctx.request_repaint();
    }

    /// Hides the window (no taskbar button; only the tray icon remains).
    pub fn hide(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.hidden = true;
        ctx.request_repaint();
    }

    /// Hidden or minimized → show; on screen → hide.
    pub fn toggle(&mut self, ctx: &egui::Context) {
        if self.on_screen(ctx) {
            self.hide(ctx);
        } else {
            self.show(ctx);
        }
    }
}

/// Compact speed for tooltips: `"5.3 MB/s"`, `"420 KB/s"`, `"0 B/s"`.
pub fn speed(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 5] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    let mut v = if bytes_per_sec.is_finite() { bytes_per_sec.max(0.0) } else { 0.0 };
    let mut unit = 0;
    // Switch units at 1000 so it never says "1010 KB/s".
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 || v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

/// Windows keeps at most 127 UTF-16 units of a tray tooltip; longer text is
/// cut at a character boundary with an ellipsis.
#[cfg_attr(not(windows), allow(dead_code))]
fn clip_tooltip(text: &str) -> String {
    const MAX: usize = 127;
    if text.encode_utf16().count() <= MAX {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        if used + c.len_utf16() > MAX - 1 {
            break;
        }
        used += c.len_utf16();
        out.push(c);
    }
    out.push('…');
    out
}

/// Left-button input from the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Click {
    Up,
    Double,
}

/// Turns left-button input into activations. A double click arrives as
/// up, double, up: the second "up" is swallowed, so a double click toggles
/// the window once instead of showing and hiding it again.
#[derive(Debug, Default)]
#[cfg_attr(not(windows), allow(dead_code))]
struct ClickFilter {
    swallow_up: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl ClickFilter {
    /// `true` when this input activates the icon.
    fn feed(&mut self, click: Click) -> bool {
        match click {
            Click::Double => {
                self.swallow_up = true;
                false
            }
            Click::Up => !std::mem::take(&mut self.swallow_up),
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{Click, ClickFilter, MenuEntry, TrayEvent, clip_tooltip};
    use eframe::egui;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Mutex, Once, mpsc};
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    enum Raw {
        Click(Click),
        Menu(String),
    }

    /// Where the process-wide handlers deliver events. tray-icon and muda
    /// accept a handler only once per process, so a new [`Tray`] (the setting
    /// turned off and on again) re-points the sink instead.
    struct Sink {
        generation: u64,
        tx: mpsc::Sender<Raw>,
        ctx: egui::Context,
    }

    static SINK: Mutex<Option<Sink>> = Mutex::new(None);
    static HANDLERS: Once = Once::new();
    static GENERATION: AtomicU64 = AtomicU64::new(0);
    /// Icons created so far. tray-icon numbers its icons 1, 2, 3… per process
    /// (the shell's `uID`), which the balloon needs; this mirrors that count.
    static CREATED: AtomicU32 = AtomicU32::new(0);

    fn deliver(raw: Raw) {
        let target = SINK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|s| (s.tx.clone(), s.ctx.clone()));
        if let Some((tx, ctx)) = target
            && tx.send(raw).is_ok()
        {
            // Wakes eframe even while the window is hidden, so `logic()` runs.
            ctx.request_repaint();
        }
    }

    fn install_handlers() {
        HANDLERS.call_once(|| {
            TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
                let click = match event {
                    TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                        Click::Up
                    }
                    TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => Click::Double,
                    // Mouse moves, other buttons (tray-icon opens the menu on
                    // right click by itself).
                    _ => return,
                };
                deliver(Raw::Click(click));
            }));
            MenuEvent::set_event_handler(Some(|event: MenuEvent| deliver(Raw::Menu(event.id.0))));
        });
    }

    /// The tray icon. Must be created and used on the main (UI) thread.
    pub struct Tray {
        icon: TrayIcon,
        /// `(id, item, current label)` of every clickable menu entry.
        items: Vec<(&'static str, MenuItem, String)>,
        rx: mpsc::Receiver<Raw>,
        clicks: ClickFilter,
        tooltip: String,
        generation: u64,
        /// The shell's id of this icon: guessed first, confirmed by the first balloon.
        uid: Cell<u32>,
        uid_confirmed: Cell<bool>,
    }

    impl Tray {
        /// Adds the icon. `rgba` is `size`×`size` RGBA pixels.
        pub fn new(
            ctx: &egui::Context,
            rgba: &[u8],
            size: u32,
            tooltip: &str,
            entries: &[MenuEntry],
        ) -> Result<Self, String> {
            install_handlers();
            let menu = Menu::new();
            let mut items = Vec::new();
            for entry in entries {
                match entry {
                    MenuEntry::Item { id, label } => {
                        let item = MenuItem::with_id(*id, label, true, None);
                        menu.append(&item).map_err(|e| e.to_string())?;
                        items.push((*id, item, label.clone()));
                    }
                    MenuEntry::Separator => menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?,
                }
            }
            let image = Icon::from_rgba(rgba.to_vec(), size, size).map_err(|e| e.to_string())?;
            let tooltip = clip_tooltip(tooltip);
            let uid = CREATED.fetch_add(1, Ordering::Relaxed) + 1;
            let icon = TrayIconBuilder::new()
                .with_icon(image)
                .with_tooltip(&tooltip)
                .with_menu(Box::new(menu))
                // Left click toggles the window; the menu is on the right button.
                .with_menu_on_left_click(false)
                .build()
                .map_err(|e| e.to_string())?;
            let (tx, rx) = mpsc::channel();
            let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
            *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Sink { generation, tx, ctx: ctx.clone() });
            Ok(Self {
                icon,
                items,
                rx,
                clicks: ClickFilter::default(),
                tooltip,
                generation,
                uid: Cell::new(uid),
                uid_confirmed: Cell::new(false),
            })
        }

        /// Everything that happened since the last call (call it in `logic()`).
        pub fn poll(&mut self) -> Vec<TrayEvent> {
            let mut out = Vec::new();
            while let Ok(raw) = self.rx.try_recv() {
                match raw {
                    Raw::Click(click) => {
                        if self.clicks.feed(click) {
                            out.push(TrayEvent::Activate);
                        }
                    }
                    Raw::Menu(id) => {
                        if self.items.iter().any(|(item_id, ..)| *item_id == id) {
                            out.push(TrayEvent::Menu(id));
                        }
                    }
                }
            }
            out
        }

        /// Changes the tooltip (cheap when it didn't change).
        pub fn set_tooltip(&mut self, text: &str) {
            let text = clip_tooltip(text);
            if text != self.tooltip && self.icon.set_tooltip(Some(&text)).is_ok() {
                self.tooltip = text;
            }
        }

        /// Renames a menu item (cheap when it didn't change).
        pub fn set_label(&mut self, id: &str, label: &str) {
            if let Some((_, item, current)) = self.items.iter_mut().find(|(item_id, ..)| *item_id == id)
                && current != label
            {
                item.set_text(label);
                *current = label.to_owned();
            }
        }

        /// Shows a balloon next to the icon (a notification on Windows 10/11).
        /// Returns `false` when the shell refused it.
        pub fn notify(&self, title: &str, body: &str) -> bool {
            use windows_sys::Win32::UI::Shell::{
                NIF_INFO, NIIF_RESPECT_QUIET_TIME, NIIF_USER, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
            };
            let hwnd = self.icon.window_handle();
            let send = |uid: u32| {
                let mut nid = NOTIFYICONDATAW {
                    cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: hwnd,
                    uID: uid,
                    uFlags: NIF_INFO,
                    // NIIF_USER: the balloon uses the tray icon itself.
                    dwInfoFlags: NIIF_USER | NIIF_RESPECT_QUIET_TIME,
                    ..Default::default()
                };
                copy_wide(&mut nid.szInfoTitle, title);
                copy_wide(&mut nid.szInfo, body);
                // SAFETY: `nid` is a fully initialized NOTIFYICONDATAW with the right
                // cbSize; NIM_MODIFY only reads it and fails for an unknown (hWnd, uID).
                unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) != 0 }
            };
            if send(self.uid.get()) {
                self.uid_confirmed.set(true);
                return true;
            }
            if self.uid_confirmed.get() {
                return false;
            }
            // tray-icon doesn't expose the id; the hidden window hosts exactly
            // one icon, so look for it among the first few ids.
            match (1..=64).filter(|&uid| uid != self.uid.get()).find(|&uid| send(uid)) {
                Some(uid) => {
                    self.uid.set(uid);
                    self.uid_confirmed.set(true);
                    true
                }
                None => false,
            }
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            // The icon itself is removed by `TrayIcon`'s Drop.
            let mut sink = SINK.lock().unwrap_or_else(|e| e.into_inner());
            if sink.as_ref().is_some_and(|s| s.generation == self.generation) {
                *sink = None;
            }
        }
    }

    /// Copies `s` into a fixed, NUL-terminated UTF-16 buffer (cut if too long).
    fn copy_wide(dst: &mut [u16], s: &str) {
        let room = dst.len().saturating_sub(1);
        let units: Vec<u16> = s.encode_utf16().take(room).collect();
        dst[..units.len()].copy_from_slice(&units);
        dst[units.len()..].fill(0);
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{MenuEntry, TrayEvent};
    use eframe::egui;

    /// No tray icon on this platform: [`Tray::new`] always fails.
    pub struct Tray {
        _private: (),
    }

    impl Tray {
        pub fn new(
            _ctx: &egui::Context,
            _rgba: &[u8],
            _size: u32,
            _tooltip: &str,
            _entries: &[MenuEntry],
        ) -> Result<Self, String> {
            Err("The tray icon is only available on Windows.".into())
        }

        pub fn poll(&mut self) -> Vec<TrayEvent> {
            Vec::new()
        }

        pub fn set_tooltip(&mut self, _text: &str) {}

        pub fn set_label(&mut self, _id: &str, _label: &str) {}

        pub fn notify(&self, _title: &str, _body: &str) -> bool {
            false
        }
    }
}

pub use imp::Tray;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_decision() {
        use CloseAction::*;
        // Only an unprompted close with the tray icon shown and the setting on hides.
        assert_eq!(close_action(false, true, true), HideToTray);
        assert_eq!(close_action(false, true, false), Exit);
        assert_eq!(close_action(false, false, true), Exit, "nothing to come back from without an icon");
        assert_eq!(close_action(false, false, false), Exit);
        // Quitting for real always exits.
        assert_eq!(close_action(true, true, true), Exit);
        assert_eq!(close_action(true, false, false), Exit);
    }

    #[test]
    fn double_click_toggles_once() {
        let mut f = ClickFilter::default();
        assert!(f.feed(Click::Up), "single click");
        assert!(f.feed(Click::Up), "another single click");
        // Double click: up, double, up.
        assert!(f.feed(Click::Up));
        assert!(!f.feed(Click::Double));
        assert!(!f.feed(Click::Up));
        assert!(f.feed(Click::Up), "next click works again");
    }

    #[test]
    fn speeds() {
        assert_eq!(speed(0.0), "0 B/s");
        assert_eq!(speed(-5.0), "0 B/s");
        assert_eq!(speed(f64::NAN), "0 B/s");
        assert_eq!(speed(999.0), "999 B/s");
        assert_eq!(speed(420.0 * 1024.0), "420 KB/s");
        assert_eq!(speed(5.3 * 1024.0 * 1024.0), "5.3 MB/s");
        assert_eq!(speed(12.0 * 1024.0 * 1024.0), "12.0 MB/s");
        assert_eq!(speed(1010.0 * 1024.0), "1.0 MB/s");
        assert_eq!(speed(2.5 * 1024.0 * 1024.0 * 1024.0), "2.5 GB/s");
    }

    #[test]
    fn tooltip_is_clipped_for_the_shell() {
        assert_eq!(clip_tooltip("Zenless · idle"), "Zenless · idle");
        let long = "é".repeat(300);
        let clipped = clip_tooltip(&long);
        assert_eq!(clipped.encode_utf16().count(), 127);
        assert!(clipped.ends_with('…'));
        // Surrogate pairs are never split.
        let emoji = "😀".repeat(100);
        let clipped = clip_tooltip(&emoji);
        assert!(clipped.encode_utf16().count() <= 127);
        assert!(clipped.chars().rev().skip(1).all(|c| c == '😀'));
    }

    #[test]
    fn menu_entries() {
        assert_eq!(MenuEntry::item("quit", "Quit"), MenuEntry::Item { id: "quit", label: "Quit".into() });
        assert_ne!(MenuEntry::item("quit", "Quit"), MenuEntry::Separator);
    }
}
