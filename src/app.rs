//! The main window.

use crate::audio::{self, AudioBand, AudioSource};
use crate::color::{BlendMode, Rgb};
use crate::config::{self, Config};
use crate::devices::{DeviceConfig, DimVia, OutputMode, SingleSample};
use crate::effects::{self, EffectKind, Layer, ModShape, ModSource, ModTarget, P, Program};
use crate::engine::{self, Preview, Shared};
use crate::govee::{self, Incoming, Net};
use crate::presets;
use crate::ui_widgets::{self as w, ACCENT, ACCENT2, DIM, LIVE};
use eframe::egui::{self, Color32, RichText, vec2};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Editor,
    Devices,
    Lab,
}

struct Lab {
    target: String,
    json: String,
    hex: String,
    checksum: bool,
    razer: bool,
    log: VecDeque<(Instant, bool, String)>,
}

pub struct App {
    shared: Arc<Mutex<Shared>>,
    engine: Option<std::thread::JoinHandle<()>>,
    cfg: Config,
    pushed: Config,
    dirty_since: Option<Instant>,
    net: Option<Net>,
    rx: Option<Receiver<Incoming>>,
    net_note: Option<String>,
    preview: Preview,
    live: bool,
    tab: Tab,
    sel_layer: Option<u64>,
    sel_device: Option<String>,
    stop_sel: HashMap<u64, usize>,
    builtins: Vec<Program>,
    user: Vec<Program>,
    preset_dir: Option<PathBuf>,
    save_name: String,
    import_text: String,
    toast: Option<(String, Instant)>,
    taps: Vec<Instant>,
    lab: Lab,
    add_ip: String,
    audio_devices: Vec<String>,
    status: HashMap<String, Value>,
    blackout: bool,
    pre_blackout: f32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        w::apply_theme(&cc.egui_ctx);
        let builtins = presets::builtins();
        let mut cfg = config::load().unwrap_or_default();
        cfg.program = fresh_ids(if cfg.program.layers.is_empty() {
            builtins[0].clone()
        } else {
            cfg.program
        });
        let iface = cfg.settings.iface.parse::<Ipv4Addr>().ok();
        let (net, rx, net_note) = match Net::open(iface) {
            Ok((n, rx)) => {
                let note = (!n.listening).then(|| {
                    "UDP port 4002 is in use (is Govee Desktop running?). Lights can still be controlled, but scanning won't find them. Add them by IP instead.".to_string()
                });
                (Some(n), Some(rx), note)
            }
            Err(e) => (None, None, Some(format!("Network unavailable: {e}"))),
        };
        if let Some(n) = &net {
            let _ = n.scan();
        }
        let shared = Arc::new(Mutex::new(Shared {
            config: cfg.clone(),
            version: 0,
            live: false,
            preview: Preview::default(),
            quit: false,
        }));
        let engine = engine::spawn(shared.clone(), net.clone());
        let preset_dir = presets::user_dir();
        let user = preset_dir
            .as_deref()
            .map(presets::load_user)
            .unwrap_or_default();
        let sel_layer = cfg.program.layers.last().map(|l| l.id);
        Self {
            shared,
            engine: Some(engine),
            pushed: cfg.clone(),
            cfg,
            dirty_since: None,
            net,
            rx,
            net_note,
            preview: Preview::default(),
            live: false,
            tab: Tab::Editor,
            sel_layer,
            sel_device: None,
            stop_sel: HashMap::new(),
            builtins,
            user,
            preset_dir,
            save_name: String::new(),
            import_text: String::new(),
            toast: None,
            taps: Vec::new(),
            lab: Lab {
                target: String::new(),
                json: r#"{"msg":{"cmd":"devStatus","data":{}}}"#.into(),
                hex: "33 05 15 01 FF 00 80 00 00 00 00 00 FF 7F".into(),
                checksum: true,
                razer: false,
                log: VecDeque::new(),
            },
            add_ip: String::new(),
            audio_devices: audio::input_device_names(),
            status: HashMap::new(),
            blackout: false,
            pre_blackout: 1.0,
        }
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn log(&mut self, outgoing: bool, s: String) {
        self.lab.log.push_front((Instant::now(), outgoing, s));
        self.lab.log.truncate(300);
    }

    fn send_to(&mut self, ip: &str, body: &Value) {
        let Some(net) = &self.net else {
            self.toast("Network unavailable");
            return;
        };
        match ip.parse::<IpAddr>() {
            Ok(addr) => {
                let r = net.send(addr, body);
                let text = format!("→ {ip}  {}", body);
                self.log(true, text);
                if let Err(e) = r {
                    self.toast(format!("Send failed: {e}"));
                }
            }
            Err(_) => self.toast(format!("'{ip}' isn't a valid IP address")),
        }
    }

    fn poll_network(&mut self) {
        let Some(rx) = &self.rx else { return };
        let msgs: Vec<Incoming> = rx.try_iter().collect();
        for m in msgs {
            match m.cmd.as_str() {
                "scan" => {
                    let ip = m.data["ip"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| m.from.to_string());
                    let id = m.data["device"].as_str().unwrap_or(&ip).to_string();
                    let sku = m.data["sku"].as_str().unwrap_or("?").to_string();
                    if let Some(d) = self
                        .cfg
                        .devices
                        .iter_mut()
                        .find(|d| d.id == id || (d.id.starts_with("ip:") && d.ip == ip))
                    {
                        d.ip = ip.clone();
                        if d.id.starts_with("ip:") {
                            d.id = id.clone();
                            if d.sku.is_empty() || d.sku == "?" {
                                d.sku = sku.clone();
                                d.mode = OutputMode::guess(&sku);
                                d.rate_hz = d.mode.default_rate();
                            }
                        }
                    } else {
                        self.cfg
                            .devices
                            .push(DeviceConfig::discovered(&id, &sku, &ip));
                        self.toast(format!("Found {sku} at {ip}"));
                    }
                }
                "devStatus" => {
                    self.status.insert(m.from.to_string(), m.data.clone());
                }
                _ => {}
            }
            self.log(false, format!("← {}  {}", m.from, m.raw));
        }
    }

    fn push_config(&mut self) {
        if self.cfg != self.pushed {
            let mut s = self.shared.lock().unwrap();
            s.config = self.cfg.clone();
            s.version += 1;
            drop(s);
            self.pushed = self.cfg.clone();
            self.dirty_since.get_or_insert(Instant::now());
        }
        if self
            .dirty_since
            .is_some_and(|t| t.elapsed() > Duration::from_secs(2))
        {
            config::save(&self.cfg);
            self.dirty_since = None;
        }
    }

    fn tap(&mut self) {
        let now = Instant::now();
        self.taps
            .retain(|t| now.duration_since(*t) < Duration::from_secs(3));
        self.taps.push(now);
        if self.taps.len() >= 2 {
            let span = now.duration_since(self.taps[0]).as_secs_f32();
            let bpm = 60.0 * (self.taps.len() - 1) as f32 / span;
            self.cfg.settings.bpm = bpm.clamp(40.0, 220.0).round();
        }
        self.cfg.beat_resync += 1;
    }

    // ------------------------------------------------------------------
    // Top bar
    // ------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        // Wraps onto a second row on narrow windows.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("◉").size(22.0).color(ACCENT));
            ui.label(
                RichText::new("ReadySetGlow")
                    .size(18.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(8.0);
            for (t, label) in [
                (Tab::Editor, "🎨 Editor"),
                (Tab::Devices, "💡 Devices"),
                (Tab::Lab, "🔬 Lab"),
            ] {
                if ui
                    .add(egui::Button::selectable(
                        self.tab == t,
                        RichText::new(label).size(14.0),
                    ))
                    .clicked()
                {
                    self.tab = t;
                }
            }
            ui.separator();
            ui.spacing_mut().slider_width = 90.0;

            let (txt, fill) = if self.live {
                ("● LIVE", LIVE.gamma_multiply(0.75))
            } else {
                ("○ Go live", w::RAISED)
            };
            let live_btn =
                egui::Button::new(RichText::new(txt).strong().size(15.0).color(Color32::WHITE))
                    .fill(fill)
                    .min_size(vec2(96.0, 30.0));
            if ui
                .add(live_btn)
                .on_hover_text("Send the effect to your lights (Space)")
                .clicked()
            {
                self.live = !self.live;
            }
            let bo = egui::Button::new(RichText::new("■ Blackout").color(if self.blackout {
                Color32::BLACK
            } else {
                Color32::WHITE
            }))
            .fill(if self.blackout {
                Color32::from_rgb(255, 210, 60)
            } else {
                w::RAISED
            })
            .min_size(vec2(0.0, 30.0));
            if ui
                .add(bo)
                .on_hover_text("Fade everything to black, keep streaming (B)")
                .clicked()
            {
                self.toggle_blackout();
            }
            ui.separator();

            ui.label(RichText::new("Master").color(DIM));
            ui.add(egui::Slider::new(&mut self.cfg.settings.master, 0.0..=1.0).show_value(false));
            ui.label(RichText::new("Speed").color(DIM));
            ui.add(
                egui::Slider::new(&mut self.cfg.settings.speed, 0.05..=4.0)
                    .logarithmic(true)
                    .fixed_decimals(2)
                    .suffix("×"),
            );
            ui.separator();

            w::beat_dots(ui, self.preview.beats);
            let mut bpm = self.cfg.settings.bpm;
            let shown_bpm = if self.cfg.settings.follow_audio_tempo
                && self.cfg.settings.audio != AudioSource::Off
            {
                self.preview.bpm
            } else {
                bpm
            };
            if ui
                .add(
                    egui::DragValue::new(&mut bpm)
                        .range(40.0..=220.0)
                        .speed(0.3)
                        .fixed_decimals(0)
                        .suffix(" bpm"),
                )
                .on_hover_text(format!(
                    "Tempo for synced layers (engine: {shown_bpm:.0} bpm)"
                ))
                .changed()
            {
                self.cfg.settings.bpm = bpm;
            }
            if ui
                .button("Tap")
                .on_hover_text("Tap along to set the tempo (T)")
                .clicked()
            {
                self.tap();
            }
            ui.separator();

            ui.label(RichText::new("Audio").color(DIM));
            let mut src = self.cfg.settings.audio.clone();
            egui::ComboBox::from_id_salt("audio-src")
                .selected_text(src.label())
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut src, AudioSource::Off, "Off");
                    ui.selectable_value(&mut src, AudioSource::Demo, AudioSource::Demo.label());
                    ui.selectable_value(&mut src, AudioSource::System, AudioSource::System.label());
                    for n in &self.audio_devices {
                        ui.selectable_value(&mut src, AudioSource::Input(n.clone()), n);
                    }
                    if ui.small_button("↻ Refresh devices").clicked() {
                        self.audio_devices = audio::input_device_names();
                    }
                });
            self.cfg.settings.audio = src;
            if self.cfg.settings.audio != AudioSource::Off {
                w::spectrum(ui, &self.preview.audio, vec2(70.0, 22.0));
                ui.checkbox(&mut self.cfg.settings.follow_audio_tempo, "Follow beat")
                    .on_hover_text("Let detected beats drive the tempo clock");
            }

            if self.preview.fps < 50.0 && self.preview.fps > 0.0 {
                ui.label(
                    RichText::new(format!("{:.0} fps", self.preview.fps))
                        .small()
                        .color(DIM),
                )
                .on_hover_text("The effect engine is running slower than 60 fps");
            }
        });
        if !self.preview.audio_status.is_empty() && self.preview.audio_status.starts_with("Audio") {
            ui.label(
                RichText::new(&self.preview.audio_status)
                    .small()
                    .color(Color32::from_rgb(255, 170, 80)),
            );
        }
    }

    fn toggle_blackout(&mut self) {
        self.blackout = !self.blackout;
        if self.blackout {
            self.pre_blackout = self.cfg.settings.master.max(0.05);
            self.cfg.settings.master = 0.0;
        } else {
            self.cfg.settings.master = self.pre_blackout;
        }
    }

    // ------------------------------------------------------------------
    // Editor tab
    // ------------------------------------------------------------------

    fn presets_panel(&mut self, ui: &mut egui::Ui) {
        w::section(ui, "Presets");
        egui::ScrollArea::vertical()
            .id_salt("presets")
            .max_height(ui.available_height() - 210.0)
            .show(ui, |ui| {
                let mut load: Option<Program> = None;
                for p in &self.builtins {
                    let active = p.name == self.cfg.program.name;
                    if ui
                        .add_sized(
                            [ui.available_width(), 24.0],
                            egui::Button::selectable(active, &p.name),
                        )
                        .clicked()
                    {
                        load = Some(p.clone());
                    }
                }
                if !self.user.is_empty() {
                    w::section(ui, "Mine");
                }
                let mut delete: Option<String> = None;
                for p in &self.user {
                    ui.horizontal(|ui| {
                        let active = p.name == self.cfg.program.name;
                        if ui
                            .add_sized(
                                [(ui.available_width() - 40.0).max(60.0), 24.0],
                                egui::Button::selectable(active, &p.name),
                            )
                            .clicked()
                        {
                            load = Some(p.clone());
                        }
                        if ui
                            .small_button("🗑")
                            .on_hover_text("Delete preset")
                            .clicked()
                        {
                            delete = Some(p.name.clone());
                        }
                    });
                }
                if let Some(p) = load {
                    let p = fresh_ids(p);
                    self.sel_layer = p.layers.last().map(|l| l.id);
                    self.save_name = p.name.clone();
                    self.cfg.program = p;
                }
                if let (Some(n), Some(dir)) = (delete, &self.preset_dir) {
                    let _ = presets::delete_user(dir, &n);
                    self.user = presets::load_user(dir);
                }
            });

        ui.separator();
        w::section(ui, "Save");
        ui.horizontal(|ui| {
            if self.save_name.is_empty() {
                self.save_name = self.cfg.program.name.clone();
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.save_name)
                    .desired_width((ui.available_width() - 70.0).max(60.0)),
            );
            if ui.button("Save").clicked() {
                let mut p = self.cfg.program.clone();
                p.name = self.save_name.trim().to_string();
                if p.name.is_empty() || self.builtins.iter().any(|b| b.name == p.name) {
                    p.name = format!(
                        "{} (mine)",
                        if p.name.is_empty() { "Preset" } else { &p.name }
                    );
                }
                match &self.preset_dir {
                    Some(dir) => match presets::save_user(dir, &p) {
                        Ok(()) => {
                            self.cfg.program.name = p.name.clone();
                            self.user = presets::load_user(dir);
                            self.toast(format!("Saved \"{}\"", p.name));
                        }
                        Err(e) => self.toast(format!("Couldn't save: {e}")),
                    },
                    None => self.toast("No config folder available"),
                }
            }
        });
        w::section(ui, "Share");
        ui.horizontal(|ui| {
            if ui
                .button("📋 Copy code")
                .on_hover_text("Copy this preset as a short code you can paste anywhere")
                .clicked()
            {
                ui.ctx()
                    .copy_text(presets::to_share_code(&self.cfg.program));
                self.toast("Preset code copied");
            }
            if ui.button("Import").clicked() {
                match presets::from_share_code(&self.import_text) {
                    Ok(p) => {
                        self.sel_layer = p.layers.last().map(|l| l.id);
                        self.toast(format!("Imported \"{}\"", p.name));
                        self.cfg.program = p;
                        self.import_text.clear();
                    }
                    Err(e) => self.toast(format!("Import failed: {e}")),
                }
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.import_text)
                .hint_text("paste rsglow:… code")
                .desired_width(f32::INFINITY),
        );
    }

    fn stage(&mut self, ui: &mut egui::Ui) {
        let canvas: Vec<Color32> = self
            .preview
            .canvas
            .iter()
            .map(|&c| w::c32(c * self.cfg.settings.master))
            .collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.cfg.program.name).size(20.0).strong());
            ui.label(RichText::new(format!("{} layers", self.cfg.program.layers.len())).color(DIM));
        });
        w::led_strip(ui, &canvas, 58.0);
        ui.add_space(4.0);

        // The honest preview: what each light will actually show.
        if self.cfg.devices.is_empty() {
            ui.label(RichText::new("No lights yet. Open 💡 Devices to scan or add one by IP. The canvas above still previews your effect.").color(DIM));
        } else {
            egui::ScrollArea::horizontal()
                .id_salt("dev-strip")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for d in &self.cfg.devices {
                            let shown = self
                                .preview
                                .devices
                                .get(&d.id)
                                .map(|p| p.shown.clone())
                                .unwrap_or_default();
                            ui.vertical(|ui| {
                                let size = match d.mode {
                                    OutputMode::Segments { .. } | OutputMode::Stream { .. } => {
                                        vec2(150.0, 22.0)
                                    }
                                    _ => vec2(44.0, 44.0),
                                };
                                let r = w::device_preview(ui, &d.mode, &shown, size, d.enabled)
                                    .on_hover_text(format!(
                                        "{} · {} · {}",
                                        d.display_name(),
                                        d.mode.label(),
                                        d.ip
                                    ));
                                if r.clicked() {
                                    self.sel_device = Some(d.id.clone());
                                    self.tab = Tab::Devices;
                                }
                                ui.label(
                                    RichText::new(d.display_name()).small().color(if d.enabled {
                                        Color32::WHITE
                                    } else {
                                        DIM
                                    }),
                                );
                            });
                            ui.add_space(10.0);
                        }
                    });
                });
        }
        ui.separator();
        self.layer_stack(ui);
    }

    fn layer_stack(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Layers").size(16.0).strong());
            ui.label(
                RichText::new("top of the list draws on top")
                    .small()
                    .color(DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut add: Option<EffectKind> = None;
                ui.menu_button(RichText::new("➕ Add layer").color(ACCENT2), |ui| {
                    ui.set_min_width(260.0);
                    for group in ["Basics", "Motion", "Nature", "Audio"] {
                        ui.label(RichText::new(group).small().strong().color(DIM));
                        for k in effects::KINDS.iter().filter(|k| k.group == group) {
                            if ui.button(k.name).on_hover_text(k.blurb).clicked() {
                                add = Some(k.kind);
                                ui.close();
                            }
                        }
                        ui.separator();
                    }
                });
                if let Some(k) = add {
                    let l = Layer::new(k);
                    self.sel_layer = Some(l.id);
                    self.cfg.program.layers.push(l);
                }
            });
        });

        let n = self.cfg.program.layers.len();
        let mut action: Option<(usize, &str)> = None;
        egui::ScrollArea::vertical()
            .id_salt("layers")
            .show(ui, |ui| {
                for idx in (0..n).rev() {
                    let l = &mut self.cfg.program.layers[idx];
                    let sel = self.sel_layer == Some(l.id);
                    let frame = egui::Frame::new()
                        .fill(if sel { w::RAISED } else { w::BG })
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if sel { ACCENT } else { w::LINE },
                        ))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(8, 6));
                    let resp = frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let eye = if l.enabled { "👁" } else { "◌" };
                            if ui
                                .add(egui::Button::new(eye).frame(false))
                                .on_hover_text("Show/hide")
                                .clicked()
                            {
                                l.enabled = !l.enabled;
                            }
                            let info = effects::info(l.kind);
                            if info.params.contains(&P::Palette) {
                                w::gradient_swatch(ui, &l.params.palette, vec2(28.0, 14.0));
                            } else {
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(28.0, 14.0), egui::Sense::hover());
                                ui.painter().rect_filled(r, 3, w::c32(l.params.color));
                            }
                            let name = RichText::new(&l.name).strong().color(if l.enabled {
                                Color32::WHITE
                            } else {
                                DIM
                            });
                            if ui
                                .add(egui::Label::new(name).sense(egui::Sense::click()))
                                .clicked()
                            {
                                self.sel_layer = Some(l.id);
                            }
                            if l.name != info.name {
                                ui.label(RichText::new(info.name).small().color(DIM));
                            }
                            if l.tempo_sync {
                                ui.label(RichText::new("♩").color(ACCENT2))
                                    .on_hover_text("Tempo-synced");
                            }
                            if l.modulator.source != ModSource::Off {
                                ui.label(RichText::new("〰").color(ACCENT))
                                    .on_hover_text("Modulated");
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("🗙").on_hover_text("Delete").clicked() {
                                        action = Some((idx, "del"));
                                    }
                                    if ui.small_button("🗐").on_hover_text("Duplicate").clicked()
                                    {
                                        action = Some((idx, "dup"));
                                    }
                                    if ui
                                        .add_enabled(idx > 0, egui::Button::new("▼").small())
                                        .on_hover_text("Move down")
                                        .clicked()
                                    {
                                        action = Some((idx, "down"));
                                    }
                                    if ui
                                        .add_enabled(idx + 1 < n, egui::Button::new("▲").small())
                                        .on_hover_text("Move up")
                                        .clicked()
                                    {
                                        action = Some((idx, "up"));
                                    }
                                    ui.add(
                                        egui::Slider::new(&mut l.opacity, 0.0..=1.0)
                                            .show_value(false),
                                    )
                                    .on_hover_text("Opacity");
                                    egui::ComboBox::from_id_salt(("blend", l.id))
                                        .selected_text(l.blend.label())
                                        .width(80.0)
                                        .show_ui(ui, |ui| {
                                            for b in BlendMode::ALL {
                                                ui.selectable_value(&mut l.blend, b, b.label());
                                            }
                                        });
                                },
                            );
                        });
                    });
                    if resp.response.interact(egui::Sense::click()).clicked() {
                        self.sel_layer = Some(self.cfg.program.layers[idx].id);
                    }
                    ui.add_space(3.0);
                }
                if n == 0 {
                    ui.label(
                        RichText::new("Empty program: add a layer to get started.").color(DIM),
                    );
                }
            });

        if let Some((i, what)) = action {
            let layers = &mut self.cfg.program.layers;
            match what {
                "del" => {
                    layers.remove(i);
                }
                "dup" => {
                    let mut c = layers[i].clone();
                    c.id = effects::next_id();
                    c.name = format!("{} copy", c.name);
                    self.sel_layer = Some(c.id);
                    layers.insert(i + 1, c);
                }
                "up" => layers.swap(i, i + 1),
                "down" => layers.swap(i, i - 1),
                _ => {}
            }
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.sel_layer else {
            ui.label(RichText::new("Select a layer to edit it.").color(DIM));
            return;
        };
        let Some(l) = self.cfg.program.layers.iter_mut().find(|l| l.id == id) else {
            ui.label(RichText::new("Select a layer to edit it.").color(DIM));
            return;
        };
        let stop_sel = self.stop_sel.entry(id).or_insert(0);
        egui::ScrollArea::vertical().id_salt("inspector").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut l.name).font(egui::TextStyle::Heading).desired_width(200.0));
            });
            // Effect type: switching keeps your colours and settings.
            let info = effects::info(l.kind);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Effect").color(DIM));
                let mut kind = l.kind;
                egui::ComboBox::from_id_salt(("kind", id)).selected_text(info.name).width(170.0).show_ui(ui, |ui| {
                    for group in ["Basics", "Motion", "Nature", "Audio"] {
                        ui.label(RichText::new(group).small().strong().color(DIM));
                        for k in effects::KINDS.iter().filter(|k| k.group == group) {
                            ui.selectable_value(&mut kind, k.kind, k.name).on_hover_text(k.blurb);
                        }
                    }
                });
                if kind != l.kind {
                    if l.name == info.name {
                        l.name = effects::info(kind).name.into();
                    }
                    l.see_through = effects::info(kind).sparse;
                    l.kind = kind;
                }
            });
            let info = effects::info(l.kind);
            ui.label(RichText::new(info.blurb).small().italics().color(DIM));
            if info.group == "Audio" {
                ui.label(RichText::new("Needs an audio source (top bar). Try \"Demo signal\" to design without music.").small().color(ACCENT2));
            }

            w::section(ui, "Look");
            let p = &mut l.params;
            for &param in info.params {
                match param {
                    P::Color => color_row(ui, "Colour", &mut p.color),
                    P::Color2 => color_row(ui, "Colour 2", &mut p.color2),
                    P::Palette => {
                        w::palette_editor(ui, &mut p.palette, stop_sel, egui::Id::new(("pal", id)));
                    }
                    P::Count => {
                        ui.horizontal(|ui| {
                            ui.label("Count");
                            ui.add(egui::Slider::new(&mut p.count, 1..=12));
                        });
                    }
                    P::Bounce => {
                        ui.checkbox(&mut p.bounce, "Bounce (back and forth)");
                    }
                    P::Mirror => {
                        ui.checkbox(&mut p.mirror, "Mirror from the centre");
                    }
                    P::Band => {
                        ui.horizontal(|ui| {
                            ui.label("Band");
                            band_combo(ui, ("band", id), &mut p.band);
                        });
                    }
                    other => {
                        let r = effects::param_range(other);
                        let v = match other {
                            P::Speed => &mut p.speed,
                            P::Scale => &mut p.scale,
                            P::Size => &mut p.size,
                            P::Density => &mut p.density,
                            P::Decay => &mut p.decay,
                            P::Intensity => &mut p.intensity,
                            _ => &mut p.gain,
                        };
                        ui.horizontal(|ui| {
                            ui.label(r.label).on_hover_text(r.help);
                            ui.add(egui::Slider::new(v, r.min..=r.max).logarithmic(r.log).max_decimals(2));
                        });
                    }
                }
            }

            w::section(ui, "Placement");
            ui.horizontal(|ui| {
                ui.label("Range");
                ui.add(egui::DragValue::new(&mut l.start).range(0.0..=1.0).speed(0.005).fixed_decimals(2));
                ui.label("→");
                ui.add(egui::DragValue::new(&mut l.end).range(0.0..=1.0).speed(0.005).fixed_decimals(2));
                ui.checkbox(&mut l.reverse, "Reverse");
            });
            ui.horizontal(|ui| {
                ui.label("Hue shift");
                ui.add(egui::Slider::new(&mut l.hue_shift, -0.5..=0.5).max_decimals(2));
            });
            ui.checkbox(&mut l.see_through, "Dark = transparent").on_hover_text("Lower layers show through the dark parts of this one");
            ui.checkbox(&mut l.tempo_sync, "Sync to tempo").on_hover_text("Speed counts in beats instead of seconds");

            w::section(ui, "Modulation");
            let m = &mut l.modulator;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("modsrc", id)).selected_text(mod_src_label(m.source)).width(110.0).show_ui(ui, |ui| {
                    for s in [ModSource::Off, ModSource::Lfo, ModSource::Tempo, ModSource::Audio, ModSource::Wander] {
                        ui.selectable_value(&mut m.source, s, mod_src_label(s));
                    }
                });
                if m.source != ModSource::Off {
                    ui.label("→");
                    egui::ComboBox::from_id_salt(("modtgt", id)).selected_text(format!("{:?}", m.target)).width(90.0).show_ui(ui, |ui| {
                        for t in [ModTarget::Opacity, ModTarget::Speed, ModTarget::Hue, ModTarget::Scale, ModTarget::Offset] {
                            ui.selectable_value(&mut m.target, t, format!("{t:?}"));
                        }
                    });
                }
            });
            if m.source != ModSource::Off {
                if matches!(m.source, ModSource::Lfo | ModSource::Tempo) {
                    ui.horizontal(|ui| {
                        ui.label("Shape");
                        for s in [ModShape::Sine, ModShape::Triangle, ModShape::Saw, ModShape::Square] {
                            ui.selectable_value(&mut m.shape, s, format!("{s:?}"));
                        }
                    });
                }
                match m.source {
                    ModSource::Audio => {
                        ui.horizontal(|ui| {
                            ui.label("Band");
                            band_combo(ui, ("modband", id), &mut m.band);
                        });
                    }
                    _ => {
                        let (label, range) = if m.source == ModSource::Tempo { ("Cycles / beat", 0.0625..=4.0) } else { ("Rate (Hz)", 0.01..=10.0) };
                        ui.horizontal(|ui| {
                            ui.label(label);
                            ui.add(egui::Slider::new(&mut m.rate, range).logarithmic(true).max_decimals(3));
                        });
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Depth");
                    ui.add(egui::Slider::new(&mut m.depth, 0.0..=1.0));
                });
            }
        });
    }

    // ------------------------------------------------------------------
    // Devices tab
    // ------------------------------------------------------------------

    fn devices_tab(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("dev-list").resizable(true).default_width(330.0).max_width(480.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("🔍 Scan network").clicked() {
                    match &self.net {
                        Some(n) => {
                            let _ = n.scan();
                            self.toast("Scanning… lights reply within a couple of seconds");
                        }
                        None => self.toast("Network unavailable"),
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.add_ip).hint_text("192.168.1.50").desired_width(130.0));
                if ui.button("Add by IP").on_hover_text("For networks that block multicast discovery").clicked() {
                    let ip = self.add_ip.trim().to_string();
                    if ip.parse::<IpAddr>().is_ok() {
                        if !self.cfg.devices.iter().any(|d| d.ip == ip) {
                            let mut d = DeviceConfig::discovered(&format!("ip:{ip}"), "", &ip);
                            d.name = format!("Light {ip}");
                            self.sel_device = Some(d.id.clone());
                            self.cfg.devices.push(d);
                        }
                        if let (Some(n), Ok(a)) = (&self.net, ip.parse()) {
                            let _ = n.probe(a);
                        }
                        self.add_ip.clear();
                    } else {
                        self.toast("That isn't a valid IP address");
                    }
                }
            });
            if let Some(n) = &self.net_note {
                ui.label(RichText::new(n).small().color(Color32::from_rgb(255, 170, 80)));
            }
            ui.separator();
            if self.cfg.devices.is_empty() {
                ui.label(RichText::new("No lights found yet.").strong());
                ui.label(RichText::new(
                    "1. In the Govee Home app, open each light's settings and turn on \"LAN Control\" (one time only).\n\
                     2. Make sure this PC is on the same network.\n\
                     3. On Windows, allow ReadySetGlow through the firewall when asked (it listens on UDP 4002).\n\
                     4. Press Scan, or add a light by IP.",
                ).small().color(DIM));
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for d in &mut self.cfg.devices {
                    let sel = self.sel_device.as_deref() == Some(d.id.as_str());
                    let shown = self.preview.devices.get(&d.id).map(|p| p.shown.clone()).unwrap_or_default();
                    let pv = self.preview.devices.get(&d.id).cloned().unwrap_or_default();
                    let frame = egui::Frame::new()
                        .fill(if sel { w::RAISED } else { w::BG })
                        .stroke(egui::Stroke::new(1.0_f32, if sel { ACCENT } else { w::LINE }))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(8, 6));
                    let r = frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut d.enabled, "").on_hover_text("Include in live output");
                            w::device_preview(ui, &OutputMode::Rgb, &[crate::devices::collapse(
                                &shown.iter().map(|c| Rgb::from_u8(c[0], c[1], c[2])).collect::<Vec<_>>(), SingleSample::Dominant).to_u8()], vec2(22.0, 22.0), d.enabled);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(d.display_name()).strong());
                                ui.label(RichText::new(format!("{} · {} · {}", if d.sku.is_empty() { "?" } else { &d.sku }, d.ip, d.mode.label())).small().color(DIM));
                                if self.live && d.enabled {
                                    let txt = match &pv.error {
                                        Some(e) => RichText::new(format!("⚠ {e}")).small().color(Color32::from_rgb(255, 120, 90)),
                                        None => RichText::new(format!("{:.1} msg/s", pv.msgs_per_sec)).small().color(LIVE),
                                    };
                                    ui.label(txt);
                                }
                            });
                        });
                    });
                    if r.response.interact(egui::Sense::click()).clicked() {
                        self.sel_device = Some(d.id.clone());
                    }
                    ui.add_space(3.0);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new("Layout").size(18.0).strong());
                ui.label(RichText::new(
                    "Every effect renders onto one long virtual strip. Place each light on it: spread them side by side and a comet travels from lamp to lamp; stack them on the same span and they all show the same thing.",
                ).small().color(DIM));
                ui.horizontal(|ui| {
                    let n = self.cfg.devices.iter().filter(|d| d.enabled).count().max(1);
                    if ui.button("↔ Spread side by side").clicked() {
                        for (i, d) in self.cfg.devices.iter_mut().filter(|d| d.enabled).enumerate() {
                            d.span_start = i as f32 / n as f32;
                            d.span_end = (i + 1) as f32 / n as f32;
                        }
                    }
                    if ui.button("☰ All the same").clicked() {
                        for d in &mut self.cfg.devices {
                            d.span_start = 0.0;
                            d.span_end = 1.0;
                        }
                    }
                    if ui.button("⇋ Mirror pairs").on_hover_text("Alternate directions, nice for lights either side of a TV or desk").clicked() {
                        for (i, d) in self.cfg.devices.iter_mut().filter(|d| d.enabled).enumerate() {
                            d.span_start = 0.0;
                            d.span_end = 1.0;
                            d.reverse = i % 2 == 1;
                        }
                    }
                });
                ui.add_space(4.0);
                let canvas: Vec<Color32> = self.preview.canvas.iter().map(|&c| w::c32(c)).collect();
                let previews = self.preview.devices.clone();
                let shown = move |id: &str| previews.get(id).map(|p| p.shown.clone()).unwrap_or_default();
                w::layout_editor(ui, &canvas, &mut self.cfg.devices, &shown, &mut self.sel_device);

                ui.separator();
                self.device_settings(ui);
            });
        });
    }

    fn device_settings(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.sel_device.clone() else {
            ui.label(RichText::new("Select a light to configure it.").color(DIM));
            return;
        };
        let Some(idx) = self.cfg.devices.iter().position(|d| d.id == id) else {
            return;
        };
        let mut send_now: Vec<Value> = Vec::new();
        let mut remove = false;
        let mut identify = false;
        let ip = self.cfg.devices[idx].ip.clone();
        let status = self.status.get(&ip).cloned();
        {
            let d = &mut self.cfg.devices[idx];
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut d.name)
                        .font(egui::TextStyle::Heading)
                        .desired_width(260.0),
                );
                ui.label(
                    RichText::new(format!("{} · {}", d.sku, d.id))
                        .small()
                        .color(DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("🗑 Forget").clicked() {
                        remove = true;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("IP");
                ui.add(egui::TextEdit::singleline(&mut d.ip).desired_width(130.0));
                if ui
                    .button("✨ Identify")
                    .on_hover_text("Flash this light")
                    .clicked()
                {
                    identify = true;
                }
                if ui
                    .button("ℹ Status")
                    .on_hover_text("Ask the light for its current state")
                    .clicked()
                {
                    send_now.push(govee::cmd_status());
                }
            });
            if let Some(s) = &status {
                ui.label(
                    RichText::new(format!("Reported: {}", s))
                        .small()
                        .monospace()
                        .color(DIM),
                );
            }

            w::section(ui, "Capability");
            ui.horizontal_wrapped(|ui| {
                let modes = [
                    (OutputMode::Stream { leds: 32, gradient: false }, "Stream", "Per-LED frames via the razer/DreamView protocol. Smoothest, if your model supports it."),
                    (OutputMode::Segments { count: 10 }, "Segments", "Per-segment colours via ptReal. Most strips, bars and floor lamps."),
                    (OutputMode::Rgb, "Single colour", "One colour for the whole light. Every RGB model."),
                    (OutputMode::White, "Tunable white", "White-only bulbs: colours become warm/cool white and brightness."),
                    (OutputMode::OnOff, "On/off", "Plugs and switches: the effect's brightness flips it on and off (max once a second)."),
                ];
                for (m, label, help) in modes {
                    let same = std::mem::discriminant(&m) == std::mem::discriminant(&d.mode);
                    if ui.add(egui::Button::selectable(same, label)).on_hover_text(help).clicked() && !same {
                        d.mode = match m {
                            OutputMode::Segments { .. } => OutputMode::Segments { count: crate::sku_table::lookup(&d.sku).map(|x| x.2).filter(|&n| n > 0).unwrap_or(10) },
                            other => other,
                        };
                        d.rate_hz = d.mode.default_rate();
                    }
                }
            });
            match &mut d.mode {
                OutputMode::Segments { count } => {
                    ui.horizontal(|ui| {
                        ui.label("Segments");
                        ui.add(egui::Slider::new(count, 1..=15));
                        ui.label(
                            RichText::new("ptReal addresses up to 15")
                                .small()
                                .color(DIM),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Packet budget").on_hover_text("Segment packets per second. Lower it if the light lags, stutters or stops responding; the biggest colour changes are always sent first.");
                        ui.add(egui::Slider::new(&mut d.packets_per_sec, 4.0..=80.0).suffix("/s").max_decimals(0));
                    });
                }
                OutputMode::Stream { leds, gradient } => {
                    ui.horizontal(|ui| {
                        ui.label("LEDs");
                        ui.add(egui::Slider::new(leds, 1..=govee::RAZER_MAX_LEDS as u16));
                        ui.checkbox(gradient, "Gradient").on_hover_text("Let the light blend between the colours you send. Good when it has more LEDs than you stream.");
                    });
                    ui.label(RichText::new("Not every model streams. If \"Test stream\" does nothing, use Segments.").small().color(DIM));
                }
                _ => {}
            }

            w::section(ui, "Output tuning");
            egui::Grid::new(("devgrid", &id))
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Max updates/s")
                        .on_hover_text("Govee lights drop or queue packets if pushed too hard");
                    ui.add(
                        egui::Slider::new(&mut d.rate_hz, 0.5..=60.0)
                            .logarithmic(true)
                            .max_decimals(1),
                    );
                    ui.end_row();
                    ui.label("Smoothing").on_hover_text(
                        "Blends between updates so slow lights glide instead of stepping",
                    );
                    ui.add(egui::Slider::new(&mut d.smoothing, 0.0..=0.95));
                    ui.end_row();
                    ui.label("Brightness trim");
                    ui.add(egui::Slider::new(&mut d.trim, 0.0..=1.5));
                    ui.end_row();
                    ui.label("Gamma")
                        .on_hover_text(">1 deepens dim colours, <1 lifts them");
                    ui.add(egui::Slider::new(&mut d.gamma, 0.4..=2.6));
                    ui.end_row();
                    if matches!(
                        d.mode,
                        OutputMode::Rgb | OutputMode::White | OutputMode::OnOff
                    ) {
                        ui.label("Collapse to one colour").on_hover_text(
                            "How the light's slice of the canvas becomes a single colour",
                        );
                        egui::ComboBox::from_id_salt(("sample", &id))
                            .selected_text(d.sample.label())
                            .show_ui(ui, |ui| {
                                for s in SingleSample::ALL {
                                    ui.selectable_value(&mut d.sample, s, s.label());
                                }
                            });
                        ui.end_row();
                    }
                    if matches!(d.mode, OutputMode::Rgb) {
                        ui.label("Dimming").on_hover_text(
                            "If dim colours look wrong or washed out, try the brightness command",
                        );
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut d.dim, DimVia::Rgb, "Scale RGB");
                            ui.selectable_value(&mut d.dim, DimVia::Brightness, "Brightness cmd");
                        });
                        ui.end_row();
                    }
                    if matches!(d.mode, OutputMode::White) {
                        ui.label("Kelvin range");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut d.min_kelvin)
                                    .range(1000.0..=10000.0)
                                    .speed(20.0)
                                    .suffix(" K"),
                            );
                            ui.label("–");
                            ui.add(
                                egui::DragValue::new(&mut d.max_kelvin)
                                    .range(1000.0..=10000.0)
                                    .speed(20.0)
                                    .suffix(" K"),
                            );
                        });
                        ui.end_row();
                    }
                    if matches!(d.mode, OutputMode::OnOff) {
                        ui.label("Switch-on level");
                        ui.add(egui::Slider::new(&mut d.onoff_threshold, 0.05..=0.95));
                        ui.end_row();
                    }
                    ui.label("Reverse");
                    ui.checkbox(&mut d.reverse, "Flip direction");
                    ui.end_row();
                });

            w::section(ui, "Direct control");
            if self.live && d.enabled {
                ui.label(
                    RichText::new(
                        "Live output is running and will override these within a moment.",
                    )
                    .small()
                    .color(ACCENT2),
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("🔌 On").clicked() {
                    send_now.push(govee::cmd_turn(true));
                }
                if ui.button("Off").clicked() {
                    send_now.push(govee::cmd_turn(false));
                }
                for (label, pct) in [("25%", 25), ("50%", 50), ("100%", 100)] {
                    if ui.button(label).clicked() {
                        send_now.push(govee::cmd_brightness(pct));
                    }
                }
                for (label, k) in [("Warm", 2700), ("Daylight", 5000), ("Cool", 6500)] {
                    if ui.button(label).clicked() {
                        send_now.push(govee::cmd_kelvin(k));
                    }
                }
                let mut c = ui
                    .data(|m| m.get_temp::<[u8; 3]>(egui::Id::new(("direct-col", &id))))
                    .unwrap_or([255, 0, 128]);
                if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                    send_now.push(govee::cmd_color(c));
                }
                ui.data_mut(|m| m.insert_temp(egui::Id::new(("direct-col", &id)), c));
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Built-in scenes").color(DIM)).on_hover_text("Run on the light itself, so they work even on models with limited LAN control. Codes vary by model.");
                for (name, code) in govee::SCENES {
                    if ui.small_button(*name).clicked() {
                        send_now.push(govee::cmd_pt_real(&[&govee::scene_packet(*code)]));
                    }
                }
            });
            if matches!(
                d.mode,
                OutputMode::Stream { .. } | OutputMode::Segments { .. }
            ) {
                ui.horizontal(|ui| {
                    if ui
                        .button("🌈 Test segments")
                        .on_hover_text("Paint a rainbow across the segments")
                        .clicked()
                    {
                        let n = match d.mode {
                            OutputMode::Segments { count } => count as usize,
                            _ => 10,
                        };
                        let packets: Vec<[u8; 20]> = (0..n.min(15))
                            .map(|i| {
                                govee::segment_color_packet(
                                    Rgb::hsv(i as f32 / n as f32, 1.0, 1.0).to_u8(),
                                    govee::segment_mask([i]),
                                )
                            })
                            .collect();
                        for ch in packets.chunks(6) {
                            let refs: Vec<&[u8]> = ch.iter().map(|p| p.as_slice()).collect();
                            send_now.push(govee::cmd_pt_real(&refs));
                        }
                    }
                    if ui
                        .button("🌈 Test stream")
                        .on_hover_text("Enable razer streaming and send a rainbow frame")
                        .clicked()
                    {
                        let leds = match d.mode {
                            OutputMode::Stream { leds, .. } => leds as usize,
                            _ => 20,
                        };
                        let frame: Vec<[u8; 3]> = (0..leds)
                            .map(|i| Rgb::hsv(i as f32 / leds as f32, 1.0, 1.0).to_u8())
                            .collect();
                        send_now.push(govee::cmd_razer(&govee::razer_enable(true)));
                        send_now.push(govee::cmd_razer(&govee::razer_frame(&frame, false)));
                    }
                    if ui.button("Stop stream").clicked() {
                        send_now.push(govee::cmd_razer(&govee::razer_enable(false)));
                    }
                });
            }
        }
        for m in send_now {
            self.send_to(&ip, &m);
        }
        if identify && let (Some(net), Ok(addr)) = (self.net.clone(), ip.parse::<IpAddr>()) {
            std::thread::spawn(move || {
                for i in 0..6 {
                    let c = if i % 2 == 0 {
                        [255, 255, 255]
                    } else {
                        [0, 0, 40]
                    };
                    let _ = net.send(addr, &govee::cmd_color(c));
                    std::thread::sleep(Duration::from_millis(280));
                }
            });
        }
        if remove {
            self.cfg.devices.remove(idx);
            self.sel_device = None;
        }
    }

    // ------------------------------------------------------------------
    // Lab tab
    // ------------------------------------------------------------------

    fn lab_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(RichText::new("Protocol lab").size(18.0).strong());
            ui.label(RichText::new("Send raw commands and watch replies. Handy for poking at models with undocumented features.").small().color(DIM));
            ui.horizontal(|ui| {
                ui.label("Target");
                egui::ComboBox::from_id_salt("lab-target").selected_text(if self.lab.target.is_empty() { "choose…".to_string() } else { self.lab.target.clone() }).show_ui(ui, |ui| {
                    for d in &self.cfg.devices {
                        ui.selectable_value(&mut self.lab.target, d.ip.clone(), format!("{} ({})", d.display_name(), d.ip));
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut self.lab.target).desired_width(130.0).hint_text("or type an IP"));
            });

            w::section(ui, "JSON (port 4003)");
            ui.add(egui::TextEdit::multiline(&mut self.lab.json).code_editor().desired_rows(3).desired_width(f32::INFINITY));
            ui.horizontal_wrapped(|ui| {
                if ui.button("Send JSON").clicked() {
                    match serde_json::from_str::<Value>(&self.lab.json) {
                        Ok(v) => {
                            let t = self.lab.target.clone();
                            self.send_to(&t, &v);
                        }
                        Err(e) => self.toast(format!("Invalid JSON: {e}")),
                    }
                }
                for (label, v) in [
                    ("devStatus", govee::cmd_status()),
                    ("turn on", govee::cmd_turn(true)),
                    ("brightness 50", govee::cmd_brightness(50)),
                    ("colour", govee::cmd_color([255, 0, 128])),
                    ("4000 K", govee::cmd_kelvin(4000)),
                ] {
                    if ui.small_button(label).clicked() {
                        self.lab.json = v.to_string();
                    }
                }
            });

            w::section(ui, "Binary packet (ptReal / razer)");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.lab.hex).code_editor().desired_width((ui.available_width() - 280.0).max(200.0)));
                ui.selectable_value(&mut self.lab.razer, false, "ptReal");
                ui.selectable_value(&mut self.lab.razer, true, "razer");
                if !self.lab.razer {
                    ui.checkbox(&mut self.lab.checksum, "Pad + XOR");
                }
            });
            let parsed = govee::parse_hex(&self.lab.hex, self.lab.checksum && !self.lab.razer);
            match &parsed {
                Ok(b) => {
                    let b = if self.lab.razer {
                        // Accept a bare razer body too: add the checksum if it's missing.
                        if b.first() == Some(&0xBB) && b.len() >= 5 && govee::xor(&b[..b.len() - 1]) == *b.last().unwrap() { b.clone() } else { let mut v = b.clone(); v.push(govee::xor(&v)); v }
                    } else {
                        b.clone()
                    };
                    ui.label(RichText::new(format!("{} bytes: {}", b.len(), govee::to_hex(&b))).small().monospace().color(DIM));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Send packet").clicked() {
                            let body = if self.lab.razer { govee::cmd_razer(&b) } else { govee::cmd_pt_real(&[&b]) };
                            let t = self.lab.target.clone();
                            self.send_to(&t, &body);
                        }
                        ui.label(RichText::new("Snippets:").color(DIM));
                        let snippets: [(&str, &str, bool); 7] = [
                            ("power on", "33 01 01", false),
                            ("brightness 30%", "33 04 1E", false),
                            ("all segments magenta", "33 05 15 01 FF 00 80 00 00 00 00 00 FF 7F", false),
                            ("segment 1 green", "33 05 15 01 00 FF 00 00 00 00 00 00 01 00", false),
                            ("scene: sunset", "33 05 04 01", false),
                            ("razer on", "BB 00 01 B1 01", true),
                            ("razer 3 LEDs", "BB 00 0B B0 00 03 FF 00 00 00 FF 00 00 00 FF", true),
                        ];
                        for (label, hex, razer) in snippets {
                            if ui.small_button(label).clicked() {
                                self.lab.hex = hex.into();
                                self.lab.razer = razer;
                                self.lab.checksum = !razer;
                            }
                        }
                    });
                }
                Err(e) => {
                    ui.label(RichText::new(format!("⚠ {e}")).small().color(Color32::from_rgb(255, 120, 90)));
                }
            }

            w::section(ui, "Traffic");
            if ui.small_button("Clear").clicked() {
                self.lab.log.clear();
            }
            egui::ScrollArea::vertical().id_salt("log").stick_to_bottom(false).show(ui, |ui| {
                for (t, out, s) in &self.lab.log {
                    let age = t.elapsed().as_secs();
                    ui.label(RichText::new(format!("{age:>4}s  {s}")).monospace().small().color(if *out { ACCENT2 } else { Color32::from_rgb(200, 255, 200) }));
                }
                if self.lab.log.is_empty() {
                    ui.label(RichText::new("Nothing yet. Replies to scans and devStatus show up here. (Live effect frames aren't logged.)").small().color(DIM));
                }
            });
        });
    }
}

fn fresh_ids(mut p: Program) -> Program {
    for l in &mut p.layers {
        l.id = effects::next_id();
    }
    p
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut Rgb) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut u = c.to_u8();
        if egui::color_picker::color_edit_button_srgb(ui, &mut u).changed() {
            *c = Rgb::from_u8(u[0], u[1], u[2]);
        }
        for hex in [
            0xff0000, 0xff8000, 0xffee00, 0x00ff40, 0x00e5ff, 0x0040ff, 0xa000ff, 0xff00aa,
            0xffffff,
        ] {
            let col = Rgb::from_hex(hex);
            let (r, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::click());
            ui.painter().rect_filled(r, 3, w::c32(col));
            if resp.clicked() {
                *c = col;
            }
        }
    });
}

fn band_combo(ui: &mut egui::Ui, id: impl std::hash::Hash, b: &mut AudioBand) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(b.label())
        .show_ui(ui, |ui| {
            for x in AudioBand::ALL {
                ui.selectable_value(b, x, x.label());
            }
        });
}

fn mod_src_label(s: ModSource) -> &'static str {
    match s {
        ModSource::Off => "No modulation",
        ModSource::Lfo => "LFO",
        ModSource::Tempo => "Tempo LFO",
        ModSource::Audio => "Audio level",
        ModSource::Wander => "Random wander",
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_network();
        {
            let mut s = self.shared.lock().unwrap();
            self.preview = s.preview.clone();
            s.live = self.live;
        }

        // Keyboard shortcuts (ignored while typing in a text field).
        if !ctx.wants_keyboard_input() {
            let (space, b, t) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::B),
                    i.key_pressed(egui::Key::T),
                )
            });
            if space {
                self.live = !self.live;
            }
            if b {
                self.toggle_blackout();
            }
            if t {
                self.tap();
            }
        }

        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(w::BG)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| self.top_bar(ui));

        if let Some((msg, t)) = &self.toast
            && t.elapsed() < Duration::from_secs(4)
        {
            egui::TopBottomPanel::bottom("toast")
                .frame(
                    egui::Frame::new()
                        .fill(w::RAISED)
                        .inner_margin(egui::Margin::symmetric(12, 6)),
                )
                .show(ctx, |ui| {
                    ui.label(RichText::new(msg).color(Color32::WHITE));
                });
        }

        match self.tab {
            Tab::Editor => {
                egui::SidePanel::left("presets")
                    .resizable(true)
                    .default_width(210.0)
                    .max_width(320.0)
                    .show(ctx, |ui| self.presets_panel(ui));
                egui::SidePanel::right("inspector")
                    .resizable(true)
                    .default_width(390.0)
                    .min_width(330.0)
                    .max_width(560.0)
                    .show(ctx, |ui| self.inspector(ui));
                egui::CentralPanel::default().show(ctx, |ui| self.stage(ui));
            }
            Tab::Devices => self.devices_tab(ctx),
            Tab::Lab => self.lab_tab(ctx),
        }

        self.push_config();
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

impl Drop for App {
    fn drop(&mut self) {
        config::save(&self.cfg);
        if let Ok(mut s) = self.shared.lock() {
            s.quit = true;
        }
        if let Some(h) = self.engine.take() {
            let _ = h.join();
        }
    }
}
