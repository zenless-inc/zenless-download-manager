//! Non-intrusive in-app notifications (bottom-right corner).

use crate::shared::theme::{Palette, alpha};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, vec2};
use egui_phosphor::regular as icons;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Success,
    Error,
    Info,
    Clipboard,
}

/// Something the user clicked in a toast.
#[derive(Clone, Debug, PartialEq)]
pub enum ToastAction {
    OpenFile(PathBuf),
    ShowInFolder(PathBuf),
    DownloadUrl(String),
    CopyText(String),
}

struct Toast {
    id: u64,
    kind: ToastKind,
    title: String,
    body: String,
    created: Instant,
    ttl: Duration,
    actions: Vec<(String, ToastAction)>,
}

#[derive(Default)]
pub struct Toasts {
    list: Vec<Toast>,
    next: u64,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, title: impl Into<String>, body: impl Into<String>, actions: Vec<(String, ToastAction)>) {
        let ttl = match kind {
            ToastKind::Clipboard => Duration::from_secs(12),
            ToastKind::Error => Duration::from_secs(9),
            _ => Duration::from_secs(7),
        };
        self.next += 1;
        self.list.push(Toast {
            id: self.next,
            kind,
            title: title.into(),
            body: body.into(),
            created: Instant::now(),
            ttl,
            actions,
        });
        // Keep the stack short.
        while self.list.len() > 4 {
            self.list.remove(0);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Draws the toasts; returns the actions clicked this frame.
    pub fn show(&mut self, ctx: &egui::Context, p: &Palette, bottom_offset: f32) -> Vec<ToastAction> {
        let now = Instant::now();
        let mut clicked = Vec::new();
        let mut dismissed: Vec<u64> = Vec::new();
        // Pause expiry while hovered.
        let hovered_any = ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::new("zdm_toast_hover"))).unwrap_or(false);
        if hovered_any {
            for t in &mut self.list {
                // Keep at least two seconds left while the pointer is over the stack.
                let keep = t.ttl.saturating_sub(Duration::from_secs(2));
                if let Some(min_created) = now.checked_sub(keep)
                    && t.created < min_created
                {
                    t.created = min_created;
                }
            }
        }
        self.list.retain(|t| now.duration_since(t.created) < t.ttl);
        if self.list.is_empty() {
            return clicked;
        }
        let mut any_hover = false;
        egui::Area::new(egui::Id::new("zdm_toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -bottom_offset))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                for t in self.list.iter().rev() {
                    let color = match t.kind {
                        ToastKind::Success => p.success,
                        ToastKind::Error => p.danger,
                        ToastKind::Info => p.info,
                        ToastKind::Clipboard => p.accent,
                    };
                    let icon = match t.kind {
                        ToastKind::Success => icons::CHECK_CIRCLE,
                        ToastKind::Error => icons::WARNING_CIRCLE,
                        ToastKind::Info => icons::INFO,
                        ToastKind::Clipboard => icons::CLIPBOARD_TEXT,
                    };
                    let age = now.duration_since(t.created).as_secs_f32();
                    let fade = (age / 0.18).min(1.0).min((t.ttl.as_secs_f32() - age) / 0.4).clamp(0.0, 1.0);
                    let shadow = ui.visuals().popup_shadow;
                    let resp = ui.scope(|ui| {
                        ui.multiply_opacity(fade);
                        egui::Frame::new()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0, alpha(color, 0.45)))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(12, 10))
                        .shadow(shadow)
                        .show(ui, |ui| {
                            ui.set_width(316.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(icon).size(20.0).color(color));
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(egui::Label::new(RichText::new(&t.title).strong().color(p.text)).truncate());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui
                                                .add(egui::Button::new(RichText::new(icons::X).color(p.text_dim)).frame(false))
                                                .on_hover_text("Dismiss")
                                                .clicked()
                                            {
                                                dismissed.push(t.id);
                                            }
                                        });
                                    });
                                    if !t.body.is_empty() {
                                        ui.add(egui::Label::new(RichText::new(&t.body).color(p.text_dim).size(12.5)).truncate());
                                    }
                                    if !t.actions.is_empty() {
                                        ui.add_space(2.0);
                                        ui.horizontal(|ui| {
                                            for (label, action) in &t.actions {
                                                if ui
                                                    .add(egui::Button::new(RichText::new(label).color(color).size(12.5)).frame(false))
                                                    .clicked()
                                                {
                                                    clicked.push(action.clone());
                                                    dismissed.push(t.id);
                                                }
                                            }
                                        });
                                    }
                                });
                            });
                        })
                        .response
                    })
                    .inner;
                    any_hover |= resp.contains_pointer();
                    ui.add_space(8.0);
                }
            });
        ctx.memory_mut(|m| m.data.insert_temp(egui::Id::new("zdm_toast_hover"), any_hover));
        self.list.retain(|t| !dismissed.contains(&t.id));
        ctx.request_repaint_after(Duration::from_millis(100));
        clicked
    }
}
