//! Polished eframe/egui dashboard for rsclipping: live daemon metrics, one-click
//! clip/record actions and a settings editor. Frameless window, square
//! corners, Inter typography — no pill buttons, no default egui look.

use crate::app::{spawn_daemon, DaemonHandle, ClipInfo};
use crate::config::{
    AppConfig, Codec, EncoderBackend, Hotkey, HotkeyKey, OverlayPosition, MOD_ALT, MOD_CTRL,
    MOD_SHIFT,
};
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Palette / theme
// ---------------------------------------------------------------------------

const BG: egui::Color32 = egui::Color32::from_rgb(0x10, 0x12, 0x18);
const PANEL: egui::Color32 = egui::Color32::from_rgb(0x16, 0x1a, 0x22);
const CARD: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x21, 0x2b);
const INPUT: egui::Color32 = egui::Color32::from_rgb(0x20, 0x27, 0x32);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x32, 0x40);
const BORDER2: egui::Color32 = egui::Color32::from_rgb(0x36, 0x40, 0x52);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xeb, 0xf2);
const WEAK: egui::Color32 = egui::Color32::from_rgb(0x8b, 0x94, 0xa7);
const FAINT: egui::Color32 = egui::Color32::from_rgb(0x5f, 0x69, 0x7a);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x5b, 0x8c, 0xff);
const ACCENT_DARK: egui::Color32 = egui::Color32::from_rgb(0x3f, 0x6f, 0xe0);
const ACCENT_GHOST: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x3a, 0x5e);
const SUCCESS: egui::Color32 = egui::Color32::from_rgb(0x48, 0xcf, 0x8e);
const DANGER: egui::Color32 = egui::Color32::from_rgb(0xff, 0x5d, 0x6e);
const DANGER_GHOST: egui::Color32 = egui::Color32::from_rgb(0x4a, 0x26, 0x2f);
const WARN: egui::Color32 = egui::Color32::from_rgb(0xf0, 0xb3, 0x4c);

const WINDOW_RADIUS: u8 = 0;
const BAR_H: f32 = 58.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Dashboard,
    Settings,
}

const INTER_R: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_M: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
const INTER_SB: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
const INTER_B: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");

fn head_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("inter_head".into()))
}

fn mono(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Monospace)
}

// ---------------------------------------------------------------------------
// Snapshot of live daemon state (copies, no locks held)
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Snapshot {
    fps: u64,
    frames: u64,
    segments: u64,
    clips: u64,
    records: u64,
    recording: bool,
    monitors: u64,
    resolution: String,
    encoder: String,
    last: String,
    error: String,
    clip_list: Vec<ClipInfo>,
    monitors_info: Vec<MonitorGui>,
}

#[derive(Clone, Default)]
pub struct MonitorGui {
    width: u32,
    height: u32,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum ToastKind {
    Ok,
    Info,
    #[allow(dead_code)]
    Warn,
    Err,
}

struct Toast {
    text: String,
    kind: ToastKind,
    at: Instant,
}

struct SettingsDraft {
    codec: Codec,
    backend: EncoderBackend,
    fps: u32,
    crf: u32,
    bitrate: u32,
    preset: String,
    history: u32,
    clip_len: u32,
    seg: f32,
    monitor: usize,
    hotkey_clip: Hotkey,
    hotkey_record_start: Hotkey,
    hotkey_record_stop: Hotkey,
    clip_dir: String,
    record_dir: String,
    max_mb: u64,
    auto_cleanup: bool,
    input_events: bool,
    input_overlay: bool,
    overlay_position: OverlayPosition,
}

/// Which hotkey slot the user is currently re-binding.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HotkeySlot {
    Clip,
    RecordStart,
    RecordStop,
}

struct GuiApp {
    slot: Arc<Mutex<Option<DaemonHandle>>>,
    draft: SettingsDraft,
    view: View,
    toast: Option<Toast>,
    capture: Option<HotkeySlot>,
    storage_bytes: u64,
    storage_ts: Instant,
    clip_list: Option<Vec<ClipInfo>>,
    closing: bool,
    test_deadline: Option<Instant>,
}

impl GuiApp {
    fn new(cc: &eframe::CreationContext<'_>, slot: Arc<Mutex<Option<DaemonHandle>>>, cfg: &AppConfig, test_deadline: Option<Instant>, start_view: View) -> Self {
        install_theme(&cc.egui_ctx);
        let draft = SettingsDraft {
            codec: cfg.encoding.codec,
            backend: cfg.encoding.backend,
            fps: cfg.encoding.fps,
            crf: cfg.encoding.crf,
            bitrate: cfg.encoding.bitrate_kbps,
            preset: crate::config::sanitize_preset(&cfg.encoding.preset),
            history: cfg.clip.history_seconds,
            clip_len: cfg.clip.default_length_seconds,
            seg: cfg.segment_seconds as f32,
            monitor: cfg.monitor_index,
            hotkey_clip: cfg.hotkey_clip,
            hotkey_record_start: cfg.hotkey_record_start,
            hotkey_record_stop: cfg.hotkey_record_stop,
            clip_dir: cfg.storage.clip_dir.display().to_string(),
            record_dir: cfg.storage.record_dir.display().to_string(),
            max_mb: cfg.storage.max_storage_mb,
            auto_cleanup: cfg.storage.auto_cleanup,
            input_events: cfg.input.capture_events,
            input_overlay: cfg.input.overlay_enabled,
            overlay_position: cfg.input.overlay_position,
        };
        GuiApp {
            slot,
            draft,
            view: start_view,
            toast: None,
            capture: None,
            storage_bytes: 0,
            storage_ts: Instant::now() - Duration::from_secs(5),
            clip_list: None,
            closing: false,
            test_deadline,
        }
    }

    fn snapshot(&self) -> Snapshot {
        let lock = self.slot.lock().unwrap();
        let mut s = Snapshot::default();
        if let Some(h) = lock.as_ref() {
            let m = &h.metrics;
            s.fps = m.fps.load(Ordering::Relaxed);
            s.frames = m.total_frames.load(Ordering::Relaxed);
            s.segments = m.segments_on_disk.load(Ordering::Relaxed);
            s.clips = m.clips_saved.load(Ordering::Relaxed);
            s.records = m.records_saved.load(Ordering::Relaxed);
            s.recording = m.recording.load(Ordering::Relaxed);
            s.monitors = m.monitors.load(Ordering::Relaxed);
            s.resolution = m.resolution.lock().unwrap().clone();
            s.encoder = m.encoder.lock().unwrap().clone();
            s.last = m.last_saved.lock().unwrap().clone();
            s.error = m.last_error.lock().unwrap().clone();
            s.monitors_info = m.monitor_list.lock().unwrap().iter()
                .map(|mi| MonitorGui { width: mi.width, height: mi.height })
                .collect();
        }
        if let Some(list) = &self.clip_list {
            s.clip_list = list.clone();
        }
        s
    }

    fn send(&self, action: crate::config::Action) {
        let lock = self.slot.lock().unwrap();
        if let Some(h) = lock.as_ref() {
            let _ = h.action_tx.send(action);
        }
    }

    fn snapshot_storage(&mut self, clip_dir: &str, record_dir: &str) {
        if self.storage_ts.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.storage_ts = Instant::now();
        let mut total: u64 = 0;
        let mut clips: Vec<ClipInfo> = Vec::new();
        for d in [clip_dir, record_dir] {
            if let Ok(entries) = std::fs::read_dir(d) {
                for e in entries.flatten() {
                    if let Ok(md) = e.path().metadata() {
                        total += md.len();
                    }
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(clip_dir) {
            for e in entries.flatten() {
                let p = e.path();
                let Ok(md) = p.metadata() else { continue };
                if !md.is_file() {
                    continue;
                }
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    clips.push(ClipInfo {
                        name: name.to_string(),
                        size: md.len(),
                        path: p.display().to_string(),
                    });
                }
            }
        }
        clips.sort_by(|a, b| b.name.cmp(&a.name));
        self.storage_bytes = total;
        self.clip_list = Some(clips);
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
    }

    fn apply_settings(&mut self, cfg: &AppConfig) {
        let mut c = cfg.clone();
        c.encoding.codec = self.draft.codec;
        c.encoding.backend = self.draft.backend;
        c.encoding.fps = self.draft.fps;
        c.encoding.crf = self.draft.crf;
        c.encoding.bitrate_kbps = self.draft.bitrate;
        c.encoding.preset = crate::config::sanitize_preset(&self.draft.preset);
        c.clip.history_seconds = self.draft.history;
        c.clip.default_length_seconds = self.draft.clip_len;
        c.segment_seconds = self.draft.seg.max(0.5) as f64;
        c.monitor_index = self.draft.monitor;
        c.hotkey_clip = self.draft.hotkey_clip;
        c.hotkey_record_start = self.draft.hotkey_record_start;
        c.hotkey_record_stop = self.draft.hotkey_record_stop;
        c.storage.clip_dir = std::path::PathBuf::from(self.draft.clip_dir.trim());
        c.storage.record_dir = std::path::PathBuf::from(self.draft.record_dir.trim());
        c.storage.max_storage_mb = self.draft.max_mb;
        c.storage.auto_cleanup = self.draft.auto_cleanup;
        c.input.capture_events = self.draft.input_events;
        c.input.overlay_enabled = self.draft.input_overlay;
        c.input.overlay_position = self.draft.overlay_position;
        match c.save() {
            Ok(()) => {
                self.toast(ToastKind::Ok, "Settings saved — restart the app for them to take effect");
            }
            Err(e) => self.toast(ToastKind::Err, format!("Save failed: {e}")),
        }
    }
}

fn install_theme(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("inter_r".into(), Arc::new(egui::FontData::from_static(INTER_R)));
    fonts
        .font_data
        .insert("inter_m".into(), Arc::new(egui::FontData::from_static(INTER_M)));
    fonts
        .font_data
        .insert("inter_sb".into(), Arc::new(egui::FontData::from_static(INTER_SB)));
    fonts
        .font_data
        .insert("inter_b".into(), Arc::new(egui::FontData::from_static(INTER_B)));
    let p = fonts.families.entry(egui::FontFamily::Proportional).or_default();
    p.insert(0, "inter_r".into());
    p.insert(1, "inter_m".into());
    p.insert(2, "inter_sb".into());
    p.insert(3, "inter_b".into());
    fonts
        .families
        .insert(egui::FontFamily::Name("inter_head".into()), vec![
            "inter_sb".into(),
            "inter_b".into(),
            "inter_r".into(),
        ]);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    style.spacing.slider_width = 220.0;
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = BG;
    style.visuals.panel_fill = BG;
    style.visuals.window_corner_radius = egui::CornerRadius::same(WINDOW_RADIUS);
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.selection.stroke = egui::Stroke::new(1.0f32, TEXT);
    style.visuals.faint_bg_color = CARD;
    style.visuals.extreme_bg_color = INPUT;
    let r = egui::CornerRadius::ZERO;
    style.visuals.widgets.noninteractive.corner_radius = r;
    style.visuals.widgets.inactive.corner_radius = r;
    style.visuals.widgets.hovered.corner_radius = r;
    style.visuals.widgets.active.corner_radius = r;
    style.visuals.widgets.inactive.bg_fill = INPUT;
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(0x26, 0x2f, 0x3d);
    style.visuals.widgets.active.bg_fill = ACCENT_DARK;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0f32, BORDER);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0f32, BORDER2);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0f32, ACCENT);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0f32, TEXT);
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0f32, TEXT);
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0f32, TEXT);
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(13.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(13.5, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(egui::TextStyle::Heading, head_font(19.0));
    style.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
    ctx.set_style(style);
}

// ---------------------------------------------------------------------------
// Small building blocks
// ---------------------------------------------------------------------------

fn modal_button(
    ui: &mut egui::Ui,
    text: &str,
    kind: ButtonKind,
    fill_width: bool,
) -> bool {
    let (fill, stroke, fg) = match kind {
        ButtonKind::Primary => (ACCENT, egui::Stroke::new(1.0f32, ACCENT), egui::Color32::WHITE),
        ButtonKind::Ghost => (egui::Color32::TRANSPARENT, egui::Stroke::new(1.0f32, BORDER2), TEXT),
        ButtonKind::Danger => (DANGER, egui::Stroke::new(1.0f32, DANGER), egui::Color32::WHITE),
        ButtonKind::DangerGhost => (DANGER_GHOST, egui::Stroke::new(1.0f32, DANGER), egui::Color32::from_rgb(0xff, 0xb3, 0xba)),
        ButtonKind::AccentGhost => (ACCENT_GHOST, egui::Stroke::new(1.0f32, ACCENT), egui::Color32::from_rgb(0xbf, 0xd2, 0xff)),
    };
    let btn = egui::Button::new(egui::RichText::new(text).font(egui::FontId::new(13.5, egui::FontFamily::Proportional)).color(fg))
        .fill(fill)
        .stroke(stroke)
.corner_radius(egui::CornerRadius::ZERO)
        .min_size(egui::vec2(if fill_width { 0.0 } else { 96.0 }, 38.0));
    if fill_width {
        ui.add_sized(egui::vec2(ui.available_width(), 38.0), btn).clicked()
    } else {
        ui.add(btn).clicked()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Ghost,
    Danger,
    #[allow(dead_code)]
    DangerGhost,
    AccentGhost,
}

fn stat_card(ui: &mut egui::Ui, title: &str, value: &str, accent: egui::Color32, width: f32) {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(egui::CornerRadius::ZERO)
        .inner_margin(egui::Margin::symmetric(14, 11))
        .stroke(egui::Stroke::new(1.0f32, BORDER))
        .show(ui, |ui| {
            ui.set_width((width - 28.0).max(100.0));
            ui.label(egui::RichText::new(title.to_uppercase()).size(10.0).color(FAINT).strong());
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(value)
                    .size(17.0)
                    .color(accent)
                    .font(egui::FontId::new(17.0, egui::FontFamily::Name("inter_head".into()))),
            );
        });
}

fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title.to_uppercase()).size(10.5).color(WEAK).strong());
    ui.add_space(4.0);
}

fn panel(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(egui::CornerRadius::ZERO)
        .inner_margin(egui::Margin::symmetric(18, 16))
        .stroke(egui::Stroke::new(1.0f32, BORDER))
        .show(ui, add_contents);
}

fn settings_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(150.0, 0.0),
            egui::Label::new(egui::RichText::new(label).color(WEAK)),
        );
        add(ui);
    });
}

fn open_folder(path: &str) {
    if path.is_empty() {
        return;
    }
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(deadline) = self.test_deadline {
            if Instant::now() >= deadline {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                self.test_deadline = None;
                return;
            }
        }
        ctx.request_repaint();
        self.poll_hotkey_capture(ctx);

        let snap = self.snapshot();
        let clip_dir = self.draft.clip_dir.clone();
        let record_dir = self.draft.record_dir.clone();
        self.snapshot_storage(&clip_dir, &record_dir);

        // Window (framed, transparent outside).
        let window_frame = egui::Frame::window(&ctx.style())
            .fill(BG)
            .corner_radius(egui::CornerRadius::same(WINDOW_RADIUS))
            .stroke(egui::Stroke::new(1.0f32, BORDER2))
            .inner_margin(egui::Margin::ZERO);

        egui::CentralPanel::default()
            .frame(window_frame)
            .show(ctx, |ui| {
                self.draw_title_bar(ui);
                match self.view {
                    View::Dashboard => self.draw_dashboard(ui, &snap),
                    View::Settings => self.draw_settings(ui, &snap),
                }
                self.draw_toast(ui.ctx());
            });

        // Explicit window outline. The egui frame stroke can be clipped on the
        // right edge in transparent frameless windows on some backends, so we
        // paint a guaranteed full-perimeter border on a foreground layer.
        {
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("rsclipping_window_border"),
            ));
            painter.rect_stroke(
                screen,
                egui::CornerRadius::ZERO,
                egui::Stroke::new(1.0f32, BORDER2),
                egui::StrokeKind::Inside,
            );
        }

        // Handle close from the custom button or the OS/taskbar.
        let requested = self.closing || ctx.input(|i| i.viewport().close_requested());
        if requested {
            self.stop_daemon();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl GuiApp {
    /// If a hotkey slot is in capture mode, watch for the next non-modifier
    /// key press (with its modifier state) and bind it.
    fn poll_hotkey_capture(&mut self, ctx: &egui::Context) {
        let Some(slot) = self.capture else { return };
        let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());
        for ev in &events {
            if let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = ev
            {
                if !*pressed || *repeat {
                    continue;
                }
                if *key == egui::Key::Escape {
                    self.capture = None;
                    self.toast(ToastKind::Info, "Hotkey capture cancelled");
                    return;
                }
                if let Some(key) = hotkey_key_from_egui(*key) {
                    let mut mods: u8 = 0;
                    if modifiers.alt {
                        mods |= MOD_ALT;
                    }
                    if modifiers.ctrl {
                        mods |= MOD_CTRL;
                    }
                    if modifiers.shift {
                        mods |= MOD_SHIFT;
                    }
                    // Letters, digits and Space need at least one modifier so a
                    // stray tap can't clobber a hotkey; F-keys work without one.
                    let needs_mod = matches!(
                        key,
                        HotkeyKey::Letter(_) | HotkeyKey::Digit(_) | HotkeyKey::Space
                    );
                    if needs_mod && mods == 0 {
                        continue;
                    }
                    let hk = Hotkey::new(mods, key);
                    match slot {
                        HotkeySlot::Clip => self.draft.hotkey_clip = hk,
                        HotkeySlot::RecordStart => self.draft.hotkey_record_start = hk,
                        HotkeySlot::RecordStop => self.draft.hotkey_record_stop = hk,
                    }
                    self.capture = None;
                    self.toast(ToastKind::Ok, format!("Hotkey set to {}", hk.display()));
                    return;
                }
            }
        }
    }

    /// Split-box row for one hotkey: function name left, capture box right.
    /// Returns true when the capture box was clicked.
    fn hotkey_row(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        slot: HotkeySlot,
        hk: Hotkey,
    ) -> bool {
        let listening = self.capture == Some(slot);
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.add_sized(
                egui::vec2(92.0, 26.0),
                egui::Label::new(
                    egui::RichText::new(label)
                        .size(12.0)
                        .color(if listening { ACCENT } else { TEXT }),
                ),
            );
            let text = if listening { "Press keys…".to_string() } else { hk.display() };
            let box_rect = egui::Rect::from_min_size(
                ui.cursor().min,
                egui::vec2(112.0, 26.0),
            );
            let resp = ui.interact(
                box_rect,
                ui.id().with(("hotkey_box", slot as u8)),
                egui::Sense::click(),
            );
            let fill = if listening { ACCENT_GHOST } else { INPUT };
            let stroke = if listening { ACCENT } else { BORDER };
            let fg = if listening { egui::Color32::WHITE } else { TEXT };
            ui.painter().rect_filled(box_rect, egui::CornerRadius::ZERO, fill);
            ui.painter().rect_stroke(
                box_rect,
                egui::CornerRadius::ZERO,
                egui::Stroke::new(1.0f32, stroke),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                box_rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::new(12.0, egui::FontFamily::Proportional),
                fg,
            );
            clicked = resp.clicked();
            ui.advance_cursor_after_rect(box_rect);
        });
        clicked
    }

    fn draw_title_bar(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        let bar = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(avail.x, BAR_H));

        // Drag region underneath the interactive buttons.
        let drag = ui.interact(bar, ui.id().with("drag_bar"), egui::Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let recording = self.snapshot_recording();
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            {
            let painter = ui.painter();
            painter.rect_filled(
                bar,
                egui::CornerRadius::ZERO,
                PANEL,
            );
            painter.line_segment(
                [egui::pos2(bar.left(), bar.bottom() - 1.0), egui::pos2(bar.right(), bar.bottom() - 1.0)],
                egui::Stroke::new(1.0f32, BORDER),
            );

            // Logo mark.
            let cy = bar.center().y;
            let logo = egui::Rect::from_center_size(egui::pos2(bar.left() + 26.0, cy), egui::vec2(28.0, 28.0));
            painter.rect_filled(logo, egui::CornerRadius::ZERO, ACCENT);
            painter.rect_filled(
                logo.shrink(2.0),
                egui::CornerRadius::ZERO,
                egui::Color32::from_rgb(0x6f, 0x9a, 0xff),
            );
            painter.text(
                logo.center(),
                egui::Align2::CENTER_CENTER,
                "R",
                head_font(15.0),
                egui::Color32::WHITE,
            );

            painter.text(
                egui::pos2(logo.right() + 11.0, cy - 7.0),
                egui::Align2::LEFT_BOTTOM,
                "RSClipping",
                head_font(15.0),
                TEXT,
            );
            painter.text(
                egui::pos2(logo.right() + 11.0, cy + 9.0),
                egui::Align2::LEFT_TOP,
                "rolling screen capture",
                egui::FontId::new(11.0, egui::FontFamily::Proportional),
                FAINT,
            );
            }

            // ── Interactive widgets: toggle + dropdowns ──
            let right = bar.right() - 12.0;
            let y = bar.top() + 14.0;

            let close = egui::Rect::from_min_size(egui::pos2(right - 92.0, y), egui::vec2(34.0, 30.0));
            let minimize = close.translate(egui::vec2(-46.0, 0.0));
            let dot_x = close.left() - 34.0;

            // Position controls from right to left so they sit left of the recording dot.
            let quality_w = 122.0;
            let len_w = 76.0;
            let hotkeys_w = 92.0;
            let toggle_w = 172.0;

            let quality = egui::Rect::from_min_size(egui::pos2(dot_x - 46.0 - quality_w, y), egui::vec2(quality_w, 30.0));
            let len = quality.translate(egui::vec2(-(len_w + 8.0), 0.0));
            let hotkeys = len.translate(egui::vec2(-(hotkeys_w + 8.0), 0.0));
            let toggle = hotkeys.translate(egui::vec2(-(toggle_w + 8.0), 0.0));

            // Dashboard/Settings switch knob.
            let is_settings = self.view == View::Settings;
            let toggle_resp = ui.interact(toggle, ui.id().with("view_toggle"), egui::Sense::click());
            if toggle_resp.clicked() {
                self.view = if is_settings { View::Dashboard } else { View::Settings };
            }

            // Clip quality dropdown (maps to CRF tiers).
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(quality).layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    let tier = quality_tier(self.draft.crf);
                    egui::ComboBox::from_id_salt("topbar_quality")
                        .selected_text(format!("{tier}  CRF {}", self.draft.crf))
                        .width(quality_w)
                        .show_ui(ui, |ui| {
                            for (label, crf) in [("Ultra", 14), ("High", 18), ("Medium", 23), ("Low", 28)] {
                                ui.selectable_value(&mut self.draft.crf, crf, format!("{label}  ·  CRF {crf}"));
                            }
                        });
                },
            );

            // Clip length dropdown.
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(len).layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    egui::ComboBox::from_id_salt("topbar_cliplen")
                        .selected_text(format!("{} s", self.draft.clip_len))
                        .width(len_w)
                        .show_ui(ui, |ui| {
                            for l in [15u32, 30, 45, 60, 90, 120] {
                                ui.selectable_value(&mut self.draft.clip_len, l, format!("{l} s"));
                            }
                        });
                },
            );

            // Hotkeys dropdown: interactive split-box rows with live capture.
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(hotkeys).layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    egui::ComboBox::from_id_salt("topbar_hotkeys")
                        .selected_text("Hotkeys")
                        .width(hotkeys_w)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show_ui(ui, |ui| {
                            ui.set_min_width(236.0);
                            ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
                            ui.label(
                                egui::RichText::new("Global hotkeys — click a box to rebind")
                                    .size(10.5)
                                    .color(FAINT),
                            );
                            let rows = [
                                ("Clip now", HotkeySlot::Clip, self.draft.hotkey_clip),
                                ("Record on", HotkeySlot::RecordStart, self.draft.hotkey_record_start),
                                ("Record off", HotkeySlot::RecordStop, self.draft.hotkey_record_stop),
                            ];
                            let mut clicked: Option<HotkeySlot> = None;
                            for (label, slot, hk) in rows {
                                if self.hotkey_row(ui, label, slot, hk) {
                                    clicked = Some(slot);
                                }
                            }
                            if let Some(slot) = clicked {
                                if self.capture == Some(slot) {
                                    self.capture = None;
                                } else {
                                    self.capture = Some(slot);
                                }
                            }
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new("Escape cancels · changes apply after restart")
                                    .size(10.5)
                                    .color(FAINT),
                            );
                        });
                },
            );

            {
            let painter = ui.painter();
            let cy = bar.center().y;

            // Switch knob: square track with a sliding square knob.
            let track = toggle.shrink2(egui::vec2(0.0, 4.0));
            painter.rect_filled(track, egui::CornerRadius::ZERO, CARD);
            painter.rect_stroke(track, egui::CornerRadius::ZERO, egui::Stroke::new(1.0f32, BORDER), egui::StrokeKind::Inside);
            let knob_w = toggle.width() / 2.0 - 3.0;
            let knob_x = if is_settings { toggle.right() - knob_w - 3.0 } else { toggle.left() + 3.0 };
            let knob = egui::Rect::from_min_size(egui::pos2(knob_x, toggle.top() + 3.0), egui::vec2(knob_w, toggle.height() - 6.0));
            painter.rect_filled(knob, egui::CornerRadius::ZERO, ACCENT);
            painter.text(
                egui::pos2(toggle.left() + toggle.width() / 4.0, cy),
                egui::Align2::CENTER_CENTER,
                "Dashboard",
                egui::FontId::new(11.5, egui::FontFamily::Proportional),
                if is_settings { WEAK } else { egui::Color32::WHITE },
            );
            painter.text(
                egui::pos2(toggle.left() + toggle.width() / 4.0 * 3.0, cy),
                egui::Align2::CENTER_CENTER,
                "Settings",
                egui::FontId::new(11.5, egui::FontFamily::Proportional),
                if is_settings { egui::Color32::WHITE } else { WEAK },
            );

            let hover = |rect: egui::Rect, base: egui::Color32| {
                let h = ui.ctx().input(|i| i.pointer.hover_pos());
                if let Some(h) = h {
                    if rect.contains(h) {
                        return base;
                    }
                }
                egui::Color32::TRANSPARENT
            };

            painter.rect_filled(minimize, egui::CornerRadius::ZERO, hover(minimize, egui::Color32::from_rgb(0x2a, 0x33, 0x42)));
            painter.line_segment(
                [
                    egui::pos2(minimize.center().x - 5.0, minimize.center().y + 0.0),
                    egui::pos2(minimize.center().x + 5.0, minimize.center().y + 0.0),
                ],
                egui::Stroke::new(1.4f32, WEAK),
            );

            painter.rect_filled(close, egui::CornerRadius::ZERO, hover(close, egui::Color32::from_rgb(0x5a, 0x2a, 0x33)));
            painter.line_segment(
                [
                    egui::pos2(close.center().x - 4.5, close.center().y - 4.5),
                    egui::pos2(close.center().x + 4.5, close.center().y + 4.5),
                ],
                egui::Stroke::new(1.4f32, egui::Color32::from_rgb(0xff, 0x9a, 0xa4)),
            );
            painter.line_segment(
                [
                    egui::pos2(close.center().x + 4.5, close.center().y - 4.5),
                    egui::pos2(close.center().x - 4.5, close.center().y + 4.5),
                ],
                egui::Stroke::new(1.4f32, egui::Color32::from_rgb(0xff, 0x9a, 0xa4)),
            );

            if recording {
                let dot = egui::pos2(close.left() - 34.0, cy);
                let pulse = 0.5 + 0.5 * (time_f32(ui.ctx()) * 4.0).sin();
                let a = (90.0 * pulse) as u8 + 30;
                painter.circle_filled(dot, 6.0, egui::Color32::from_rgba_unmultiplied(0xff, 0x5d, 0x6e, a));
                painter.circle_filled(dot, 3.4, DANGER);
                painter.text(
                    egui::pos2(dot.x - 12.0, cy),
                    egui::Align2::RIGHT_CENTER,
                    "Recording",
                    egui::FontId::new(12.0, egui::FontFamily::Proportional),
                    egui::Color32::from_rgb(0xff, 0x9a, 0xa4),
                );
            }

            let ptr = ui.ctx().input(|i| i.pointer.latest_pos());
            let clicked = ui.ctx().input(|i| i.pointer.primary_clicked());
            if let Some(p) = ptr {
                if clicked {
                    if minimize.contains(p) {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    } else if close.contains(p) {
                        self.closing = true;
                    }
                }
            }
            }
        });
    }

    fn snapshot_recording(&self) -> bool {
        let lock = self.slot.lock().unwrap();
        lock.as_ref()
            .map(|h| h.metrics.recording.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    fn draw_dashboard(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        egui::Frame::new()
            .fill(egui::Color32::TRANSPARENT)
            .inner_margin(egui::Margin::symmetric(24, 16))
            .show(ui, |ui| {
                section_header(ui, "Live");
                let fps = if snap.fps > 0 {
                    format!("{} fps", snap.fps)
                } else {
                    "warming…".to_string()
                };
                let items: [(&str, String, egui::Color32); 8] = [
                    ("Capture", fps, ACCENT),
                    ("Output", snap.encoder.trim().to_string(), TEXT),
                    ("Source", snap.resolution.trim().to_string(), TEXT),
                    ("Segments", snap.segments.to_string(), WEAK),
                    ("Clips", snap.clips.to_string(), SUCCESS),
                    ("Records", snap.records.to_string(), WARN),
                    ("Storage", fmt_bytes(self.storage_bytes), WEAK),
                    ("Last", fmt_last(&snap.last), FAINT),
                ];
                for chunk in items.chunks(4) {
                    let avail = ui.available_width();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        let n = chunk.len() as f32;
                        let w = ((avail - (n - 1.0) * 12.0) / n).max(120.0);
                        for (t, v, c) in chunk {
                            stat_card(ui, t, v, *c, w);
                        }
                    });
                    ui.add_space(12.0);
                }

                if !snap.error.is_empty() {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(DANGER_GHOST)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .stroke(egui::Stroke::new(1.0f32, egui::Color32::from_rgb(0x5a, 0x2a, 0x33)))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&snap.error)
                                    .size(11.0)
                                    .color(egui::Color32::from_rgb(0xff, 0x9a, 0xa4)),
                            );
                        });
                }

                ui.add_space(14.0);
                section_header(ui, "Actions");
                ui.horizontal(|ui| {
                    if snap.recording {
                        if modal_button(ui, "Stop recording", ButtonKind::Danger, false) {
                            self.send(crate::config::Action::RecordStop);
                            self.toast(ToastKind::Info, "Stopping recording…");
                        }
                    } else if modal_button(ui, "Start recording", ButtonKind::Primary, false) {
                        self.send(crate::config::Action::RecordStart);
                        self.toast(ToastKind::Info, "Recording started");
                    }
                    if modal_button(ui, "Clip now", ButtonKind::AccentGhost, false) {
                        self.send(crate::config::Action::Clip);
                        self.toast(ToastKind::Info, "Clip queued");
                    }
                    if modal_button(ui, "Open clips folder", ButtonKind::Ghost, false) {
                        open_folder(&self.draft.clip_dir);
                    }
                    if modal_button(ui, "Open recordings folder", ButtonKind::Ghost, false) {
                        open_folder(&self.draft.record_dir);
                    }
                });

                ui.add_space(14.0);

                if !snap.clip_list.is_empty() {
                    section_header(ui, "Recent Clips");
                    egui::ScrollArea::horizontal()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            for clip in snap.clip_list.iter().take(8) {
                                self.clip_card(ui, clip);
                            }
                        });
                }

                ui.add_space(14.0);
                let hint = format!(
                    "RSClipping runs in the background even with this window closed — use {} / {} / {} anywhere.",
                    self.draft.hotkey_clip.display(),
                    self.draft.hotkey_record_start.display(),
                    self.draft.hotkey_record_stop.display(),
                );
                ui.label(
                    egui::RichText::new(hint)
                        .size(11.0)
                        .color(FAINT),
                );
            });
    }

    fn clip_card(&mut self, ui: &mut egui::Ui, clip: &ClipInfo) {
        let resp = ui.allocate_response(
            egui::vec2(180.0, 110.0),
            egui::Sense::click(),
        );
        let rect = resp.rect;
        let is_hovered = resp.hovered();

        let fill = if is_hovered { INPUT } else { CARD };
        ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, fill);
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0f32, if is_hovered { ACCENT } else { BORDER }),
            egui::StrokeKind::Inside,
        );

        ui.put(
            egui::Rect::from_min_size(
                rect.min + egui::vec2(8.0, 8.0),
                egui::vec2(164.0, 94.0),
            ),
            |ui: &mut egui::Ui| {
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&clip.name)
                            .size(11.0)
                            .color(TEXT)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(fmt_bytes(clip.size))
                            .size(10.0)
                            .color(WEAK)
                            .font(mono(10.0)),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Click to play")
                            .size(9.5)
                            .color(ACCENT),
                    );
                })
                .response
            },
        );

        if resp.clicked() {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &clip.path])
                .spawn();
            self.toast(ToastKind::Info, format!("Playing {}", clip.name));
        }
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        egui::Frame::new()
            .fill(egui::Color32::TRANSPARENT)
            .inner_margin(egui::Margin::symmetric(20, 8))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(2.0);

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Settings").size(16.0).font(egui::FontId::new(16.0, egui::FontFamily::Name("inter_head".into()))).color(TEXT));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("applies after restart").size(10.5).color(FAINT));
                });
                ui.add_space(6.0);

                panel(ui, |ui| {
                    section_header(ui, "Capture");
                    settings_row(ui, "Frames per second", |ui| {
                        ui.add(egui::Slider::new(&mut self.draft.fps, 30..=240).show_value(true));
                    });
                    settings_row(ui, "Monitor", |ui| {
                        if snap.monitors_info.is_empty() {
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new("-").min_size(egui::vec2(26.0, 24.0))).clicked()
                                    && self.draft.monitor > 0
                                {
                                    self.draft.monitor -= 1;
                                }
                                ui.label(format!("{}", self.draft.monitor));
                                if ui.add(egui::Button::new("+").min_size(egui::vec2(26.0, 24.0))).clicked()
                                    && self.draft.monitor < snap.monitors.saturating_sub(1) as usize
                                {
                                    self.draft.monitor += 1;
                                }
                            });
                        } else {
                            let mut sel = self.draft.monitor.min(snap.monitors_info.len() - 1);
                            let selected_text = monitor_label(&snap.monitors_info, sel);
                            egui::ComboBox::from_id_salt("monitor_select")
                                .selected_text(selected_text)
                                .width(220.0)
                                .show_ui(ui, |ui| {
                                    for (i, _m) in snap.monitors_info.iter().enumerate() {
                                        ui.selectable_value(&mut sel, i, monitor_label(&snap.monitors_info, i));
                                    }
                                });
                            if sel != self.draft.monitor {
                                self.draft.monitor = sel;
                            }
                        }
                    });
                    settings_row(ui, "Segment length", |ui| {
                        ui.add(egui::Slider::new(&mut self.draft.seg, 1.0..=6.0).suffix(" s").show_value(true));
                    });
                });

                ui.add_space(10.0);
                panel(ui, |ui| {
                    section_header(ui, "Input capture");
                    ui.add_space(2.0);
                    ui.checkbox(
                        &mut self.draft.input_events,
                        "Save mouse / keyboard / gamepad events (\"*.inputs.json\")",
                    );
                    ui.add_space(4.0);
                    ui.checkbox(
                        &mut self.draft.input_overlay,
                        "Draw pressed keys + click rings into the video",
                    );
                    ui.add_space(4.0);
                    settings_row(ui, "Overlay position", |ui| {
                        egui::ComboBox::from_id_salt("overlay_pos")
                            .selected_text(self.draft.overlay_position.label())
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for &p in OverlayPosition::all() {
                                    ui.selectable_value(
                                        &mut self.draft.overlay_position,
                                        p,
                                        p.label(),
                                    );
                                }
                            });
                    });
                });

                ui.add_space(10.0);
                panel(ui, |ui| {
                    section_header(ui, "Encoding");
                    settings_row(ui, "Codec", |ui| {
                        egui::ComboBox::from_id_salt("codec")
                            .selected_text(codec_label(self.draft.codec))
                            .width(210.0)
                            .show_ui(ui, |ui| {
                                for c in [Codec::H264, Codec::H265, Codec::VP9, Codec::AV1] {
                                    ui.selectable_value(&mut self.draft.codec, c, codec_label(c));
                                }
                            });
                    });
                    settings_row(ui, "Backend", |ui| {
                        egui::ComboBox::from_id_salt("backend")
                            .selected_text(backend_label(self.draft.backend))
                            .width(210.0)
                            .show_ui(ui, |ui| {
                                for b in [EncoderBackend::Auto, EncoderBackend::Cpu, EncoderBackend::Gpu] {
                                    ui.selectable_value(&mut self.draft.backend, b, backend_label(b));
                                }
                            });
                    });
                    settings_row(ui, "Preset", |ui| {
                        egui::ComboBox::from_id_salt("preset")
                            .selected_text(self.draft.preset.clone())
                            .width(210.0)
                            .show_ui(ui, |ui| {
                                for p in preset_options() {
                                    ui.selectable_value(&mut self.draft.preset, p.to_string(), p.to_string());
                                }
                            });
                    });
                    settings_row(ui, "CRF / quality", |ui| {
                        ui.add(egui::Slider::new(&mut self.draft.crf, 12..=32).show_value(true));
                    });
                    settings_row(ui, "Bitrate (kbps)", |ui| {
                        ui.add(egui::Slider::new(&mut self.draft.bitrate, 0..=24000).suffix("  ·  0 = auto").show_value(true));
                    });
                });

                ui.add_space(10.0);
                panel(ui, |ui| {
                    section_header(ui, "Clips & storage");
                    settings_row(ui, "Clip history", |ui| {
                        ui.add(egui::Slider::new(&mut self.draft.history, 10..=300).suffix(" s").show_value(true));
                    });
                    settings_row(ui, "Default clip length", |ui| {
                        ui.add(egui::Slider::new(&mut self.draft.clip_len, 5..=120).suffix(" s").show_value(true));
                    });
                    settings_row(ui, "Storage budget", |ui| {
                        ui.add(egui::Slider::new(&mut self.draft.max_mb, 0..=16384).suffix(" MB  ·  0 = unlimited").show_value(true));
                    });
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.draft.auto_cleanup, "Auto-clean old clips when full");
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("OUTPUT FOLDERS").size(10.0).color(FAINT));

                    Self::folder_row(ui, "Clips", &mut self.draft.clip_dir);
                    Self::folder_row(ui, "Recordings", &mut self.draft.record_dir);
                });

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if modal_button(ui, "Apply settings", ButtonKind::Primary, false) {
                        let cfg = AppConfig::load().unwrap_or_default();
                        self.apply_settings(&cfg);
                    }
                    if modal_button(ui, "Reset", ButtonKind::Ghost, false) {
                        self.revert_draft();
                        self.toast(ToastKind::Info, "Reverted unsaved changes");
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Changing codec, fps or segment length requires a restart of the capture engine.")
                        .size(10.5)
                        .color(FAINT),
                );
                ui.add_space(8.0);
            });
            });
    }

    fn folder_row(ui: &mut egui::Ui, label: &str, path: &mut String) {
        ui.label(egui::RichText::new(label).size(11.0).color(WEAK));
        ui.horizontal(|ui| {
            let te_w = (ui.available_width() - 88.0).max(160.0);
            ui.add_sized(
                egui::vec2(te_w, 26.0),
                egui::TextEdit::singleline(path).hint_text("path"),
            );
            if ui.add(egui::Button::new("Browse…").min_size(egui::vec2(80.0, 26.0))).clicked() {
                let start = PathBuf::from(path.trim());
                if let Some(dir) = crate::utils::pick_folder(&start) {
                    *path = dir.display().to_string();
                }
            }
        });
        ui.add_space(6.0);
    }

    fn revert_draft(&mut self) {
        let cfg = AppConfig::load().unwrap_or_default();
        self.draft = SettingsDraft {
            codec: cfg.encoding.codec,
            backend: cfg.encoding.backend,
            fps: cfg.encoding.fps,
            crf: cfg.encoding.crf,
            bitrate: cfg.encoding.bitrate_kbps,
            preset: crate::config::sanitize_preset(&cfg.encoding.preset),
            history: cfg.clip.history_seconds,
            clip_len: cfg.clip.default_length_seconds,
            seg: cfg.segment_seconds as f32,
            monitor: cfg.monitor_index,
            hotkey_clip: cfg.hotkey_clip,
            hotkey_record_start: cfg.hotkey_record_start,
            hotkey_record_stop: cfg.hotkey_record_stop,
            clip_dir: cfg.storage.clip_dir.display().to_string(),
            record_dir: cfg.storage.record_dir.display().to_string(),
            max_mb: cfg.storage.max_storage_mb,
            auto_cleanup: cfg.storage.auto_cleanup,
            input_events: cfg.input.capture_events,
            input_overlay: cfg.input.overlay_enabled,
            overlay_position: cfg.input.overlay_position,
        };
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else { return };
        let age = Instant::now().duration_since(toast.at).as_secs_f64();
        let mut alpha = 1.0f32;
        if age > 3.0 {
            if age > 4.0 {
                self.toast = None;
                return;
            }
            alpha = (4.0 - age) as f32;
        }
        let fill = match toast.kind {
            ToastKind::Ok => egui::Color32::from_rgb(0x1c, 0x36, 0x2a),
            ToastKind::Info => egui::Color32::from_rgb(0x22, 0x2e, 0x46),
            ToastKind::Warn => egui::Color32::from_rgb(0x3a, 0x2e, 0x18),
            ToastKind::Err => egui::Color32::from_rgb(0x46, 0x24, 0x2c),
        };
        let color = match toast.kind {
            ToastKind::Ok => SUCCESS,
            ToastKind::Info => egui::Color32::from_rgb(0xa8, 0xbf, 0xff),
            ToastKind::Warn => WARN,
            ToastKind::Err => egui::Color32::from_rgb(0xff, 0x8a, 0x94),
        };
        let screen = ctx.screen_rect();
        let w = 320.0f32.min((screen.width() - 40.0).max(120.0));
        let rect = egui::Rect::from_center_size(
            egui::pos2(screen.center().x, screen.bottom() - 34.0),
            egui::vec2(w, 36.0),
        );
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("rsclipping_toast")));
        painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), (255.0 * alpha) as u8));
        painter.rect(
            rect,
            egui::CornerRadius::ZERO,
            egui::Color32::TRANSPARENT,
            egui::Stroke::new(1.0f32, egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (255.0 * alpha) as u8)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &toast.text,
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
            egui::Color32::from_rgba_unmultiplied(0xe8, 0xeb, 0xf2, (255.0 * alpha) as u8),
        );
    }

    fn stop_daemon(&mut self) {
        self.closing = false;
        let mut lock = self.slot.lock().unwrap();
        if let Some(h) = lock.take() {
            h.stop.store(true, Ordering::Relaxed);
            let _ = h.thread.join();
        }
    }
}

fn time_f32(ctx: &egui::Context) -> f32 {
    ctx.input(|i| i.time) as f32
}

fn preset_options() -> &'static [&'static str] {
    &["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow", "placebo"]
}

/// Map an egui `Key` to a re-bindable base key. Printable letters, digits,
/// F1..=F12 and Space are supported. `None` for keys we can't register.
fn hotkey_key_from_egui(key: egui::Key) -> Option<HotkeyKey> {
    use egui::Key::*;
    let n = |k: egui::Key| -> Option<u8> {
        match k {
            F1 => Some(1),
            F2 => Some(2),
            F3 => Some(3),
            F4 => Some(4),
            F5 => Some(5),
            F6 => Some(6),
            F7 => Some(7),
            F8 => Some(8),
            F9 => Some(9),
            F10 => Some(10),
            F11 => Some(11),
            F12 => Some(12),
            _ => None,
        }
    };
    if let Some(n) = n(key) {
        return Some(HotkeyKey::F(n));
    }
    let letter = match key {
        A => Some(b'a'),
        B => Some(b'b'),
        C => Some(b'c'),
        D => Some(b'd'),
        E => Some(b'e'),
        F => Some(b'f'),
        G => Some(b'g'),
        H => Some(b'h'),
        I => Some(b'i'),
        J => Some(b'j'),
        K => Some(b'k'),
        L => Some(b'l'),
        M => Some(b'm'),
        N => Some(b'n'),
        O => Some(b'o'),
        P => Some(b'p'),
        Q => Some(b'q'),
        R => Some(b'r'),
        S => Some(b's'),
        T => Some(b't'),
        U => Some(b'u'),
        V => Some(b'v'),
        W => Some(b'w'),
        X => Some(b'x'),
        Y => Some(b'y'),
        Z => Some(b'z'),
        _ => None,
    };
    if let Some(c) = letter {
        return Some(HotkeyKey::Letter(c));
    }
    let digit = match key {
        Num0 => Some(0),
        Num1 => Some(1),
        Num2 => Some(2),
        Num3 => Some(3),
        Num4 => Some(4),
        Num5 => Some(5),
        Num6 => Some(6),
        Num7 => Some(7),
        Num8 => Some(8),
        Num9 => Some(9),
        _ => None,
    };
    if let Some(d) = digit {
        return Some(HotkeyKey::Digit(d));
    }
    if key == Space {
        return Some(HotkeyKey::Space);
    }
    None
}

fn quality_tier(crf: u32) -> &'static str {
    if crf <= 14 {
        "Ultra"
    } else if crf <= 18 {
        "High"
    } else if crf <= 24 {
        "Medium"
    } else {
        "Low"
    }
}

fn codec_label(c: Codec) -> &'static str {
    match c {
        Codec::H264 => "H.264",
        Codec::H265 => "H.265 / HEVC",
        Codec::VP9 => "VP9",
        Codec::AV1 => "AV1",
    }
}

fn backend_label(b: EncoderBackend) -> &'static str {
    match b {
        EncoderBackend::Auto => "Auto (GPU if available)",
        EncoderBackend::Cpu => "CPU only",
        EncoderBackend::Gpu => "GPU only",
    }
}

fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes / 1024)
    }
}

fn monitor_label(mons: &[MonitorGui], i: usize) -> String {
    match mons.get(i) {
        Some(m) => format!("Display {} — {}x{}", i + 1, m.width, m.height),
        None => format!("Display {}", i + 1),
    }
}

fn fmt_last(path: &str) -> String {
    if path.is_empty() {
        return "—".to_string();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(cfg: AppConfig) -> anyhow::Result<()> {
    let handle = spawn_daemon(cfg.clone());
    let slot: Arc<Mutex<Option<DaemonHandle>>> = Arc::new(Mutex::new(Some(handle)));
    let slot_c = slot.clone();
    let base = cfg.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 660.0])
            .with_min_inner_size([820.0, 560.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_active(false)
            .with_title("RSClipping"),
        ..Default::default()
    };

    let test_seconds: Option<f32> = std::env::var("RSCLIPPING_GUI_TEST_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0.0);
    let test_deadline = test_seconds.map(|n| Instant::now() + Duration::from_secs_f32(n));

    let start_view = std::env::var("RSCLIPPING_GUI_VIEW")
        .ok()
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "settings" => View::Settings,
            _ => View::Dashboard,
        })
        .unwrap_or(View::Dashboard);

    let res = eframe::run_native(
        "RSClipping",
        options,
        Box::new(move |cc| {
            Ok(Box::new(GuiApp::new(cc, slot_c, &base, test_deadline, start_view)))
        }),
    );

    let mut lock = slot.lock().unwrap();
    if let Some(h) = lock.take() {
        h.stop.store(true, Ordering::Relaxed);
        let _ = h.thread.join();
    }

    res.map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}