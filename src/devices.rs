//! Per-device configuration and the "adaptive output" layer: turning a slice
//! of the effect canvas into the best thing each device can actually show.
//!
//! Capability tiers, from richest to most limited:
//!   Stream   - per-LED frames over the razer protocol (DreamView-capable models)
//!   Segments - per-segment colours via ptReal (most strips/bars with segments)
//!   Rgb      - one colour for the whole device (`colorwc`)
//!   White    - tunable white only: colour becomes Kelvin + brightness
//!   OnOff    - smart plugs and switches: brightness becomes on/off
//!
//! Each tier gets its own rate limit, change detection and sampling strategy,
//! so a busy effect degrades gracefully instead of flooding a bulb that can
//! only manage a few updates a second.

use crate::color::Rgb;
use crate::govee;
use crate::sku_table;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OutputMode {
    OnOff,
    White,
    Rgb,
    Segments { count: u8 },
    Stream { leds: u16, gradient: bool },
}

impl OutputMode {
    pub fn label(&self) -> String {
        match self {
            OutputMode::OnOff => "On/off only".into(),
            OutputMode::White => "Tunable white".into(),
            OutputMode::Rgb => "Single colour".into(),
            OutputMode::Segments { count } => format!("{count} segments"),
            OutputMode::Stream { leds, .. } => format!("Stream {leds} LEDs"),
        }
    }

    pub fn pixel_count(&self) -> usize {
        match self {
            OutputMode::Segments { count } => (*count).max(1) as usize,
            OutputMode::Stream { leds, .. } => {
                (*leds).clamp(1, govee::RAZER_MAX_LEDS as u16) as usize
            }
            _ => 1,
        }
    }

    pub fn default_rate(&self) -> f32 {
        match self {
            OutputMode::OnOff => 1.0,
            OutputMode::White => 4.0,
            OutputMode::Rgb => 10.0,
            OutputMode::Segments { .. } => 8.0,
            OutputMode::Stream { .. } => 30.0,
        }
    }

    /// Best guess from the SKU. Streaming is never assumed: the razer
    /// protocol isn't advertised, so the user opts in after testing it.
    pub fn guess(sku: &str) -> OutputMode {
        match sku_table::lookup(sku) {
            Some((false, _, _)) => OutputMode::OnOff,
            Some((true, _, n)) if n > 0 => OutputMode::Segments { count: n },
            _ => OutputMode::Rgb,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SingleSample {
    /// Bright pixels win, so a lone comet still shows its colour.
    #[default]
    Dominant,
    Average,
    Centre,
    Peak,
}

impl SingleSample {
    pub const ALL: [SingleSample; 4] = [
        SingleSample::Dominant,
        SingleSample::Average,
        SingleSample::Centre,
        SingleSample::Peak,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SingleSample::Dominant => "Dominant colour",
            SingleSample::Average => "Average",
            SingleSample::Centre => "Centre pixel",
            SingleSample::Peak => "Brightest pixel",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DimVia {
    /// Scale the RGB values. Fast, works on most models.
    #[default]
    Rgb,
    /// Send full-strength colour plus a separate brightness command. Slower
    /// but keeps colours accurate on models that normalise dim RGB values.
    Brightness,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    /// Govee device id (MAC-like), or `ip:<addr>` for manually added lights.
    pub id: String,
    pub name: String,
    pub sku: String,
    pub ip: String,
    pub enabled: bool,
    pub mode: OutputMode,
    /// Where this device sits on the effect canvas (fractions).
    pub span_start: f32,
    pub span_end: f32,
    pub reverse: bool,
    /// Max updates per second.
    pub rate_hz: f32,
    /// 0 = raw, 0.9 = very smooth. Hides the low update rate on slow devices.
    pub smoothing: f32,
    pub gamma: f32,
    pub trim: f32,
    pub sample: SingleSample,
    pub dim: DimVia,
    pub min_kelvin: f32,
    pub max_kelvin: f32,
    pub onoff_threshold: f32,
    /// Segment packets per second. Each ptReal packet is relayed to the
    /// light's controller one by one, so this matters more than the update rate.
    pub packets_per_sec: f32,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            sku: String::new(),
            ip: String::new(),
            enabled: true,
            mode: OutputMode::Rgb,
            span_start: 0.0,
            span_end: 1.0,
            reverse: false,
            rate_hz: 10.0,
            smoothing: 0.3,
            gamma: 1.0,
            trim: 1.0,
            sample: SingleSample::Dominant,
            dim: DimVia::Rgb,
            min_kelvin: 2000.0,
            max_kelvin: 9000.0,
            onoff_threshold: 0.35,
            packets_per_sec: 24.0,
        }
    }
}

impl DeviceConfig {
    pub fn discovered(id: &str, sku: &str, ip: &str) -> Self {
        let mode = OutputMode::guess(sku);
        Self {
            id: id.to_string(),
            name: sku.to_string(),
            sku: sku.to_string(),
            ip: ip.to_string(),
            rate_hz: mode.default_rate(),
            mode,
            ..Default::default()
        }
    }

    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.ip
        } else {
            &self.name
        }
    }
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// Box-filter the canvas span [a, b] into `count` output pixels.
pub fn sample_span(canvas: &[Rgb], a: f32, b: f32, count: usize, reverse: bool) -> Vec<Rgb> {
    let n = canvas.len();
    let mut out = vec![Rgb::BLACK; count];
    if n == 0 || count == 0 {
        return out;
    }
    let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
    let (lo, hi) = (a.min(b) * n as f32, a.max(b) * n as f32);
    let width = (hi - lo).max(1e-3);
    for (k, o) in out.iter_mut().enumerate() {
        let s0 = lo + width * k as f32 / count as f32;
        let s1 = lo + width * (k + 1) as f32 / count as f32;
        if s1 - s0 < 1.0 {
            // Narrower than a canvas pixel: point-sample with interpolation.
            let x = ((s0 + s1) * 0.5 - 0.5).clamp(0.0, (n - 1) as f32);
            let i = x.floor() as usize;
            let j = (i + 1).min(n - 1);
            *o = canvas[i].lerp(canvas[j], x - i as f32);
            continue;
        }
        let mut acc = Rgb::BLACK;
        let mut w = 0.0;
        let mut i = s0.floor() as usize;
        while (i as f32) < s1 && i < n {
            let cov = ((i + 1) as f32).min(s1) - (i as f32).max(s0);
            if cov > 0.0 {
                acc = acc + canvas[i] * cov;
                w += cov;
            }
            i += 1;
        }
        *o = if w > 0.0 { acc * (1.0 / w) } else { Rgb::BLACK };
    }
    if reverse {
        out.reverse();
    }
    out
}

/// Collapse several pixels into the one colour a single-colour bulb shows.
pub fn collapse(px: &[Rgb], how: SingleSample) -> Rgb {
    if px.is_empty() {
        return Rgb::BLACK;
    }
    match how {
        SingleSample::Average => {
            px.iter().fold(Rgb::BLACK, |a, &c| a + c) * (1.0 / px.len() as f32)
        }
        SingleSample::Centre => px[px.len() / 2],
        SingleSample::Peak => *px
            .iter()
            .max_by(|a, b| a.max_channel().total_cmp(&b.max_channel()))
            .unwrap(),
        SingleSample::Dominant => {
            let mut acc = Rgb::BLACK;
            let mut wsum = 0.0;
            let mut vmax: f32 = 0.0;
            let mut vsum = 0.0;
            for &c in px {
                let v = c.max_channel();
                let w = v * v + 1e-4;
                acc = acc + c * w;
                wsum += w;
                vmax = vmax.max(v);
                vsum += v;
            }
            let hue = acc * (1.0 / wsum);
            let target_v = 0.5 * vmax + 0.5 * vsum / px.len() as f32;
            let m = hue.max_channel();
            if m < 1e-4 {
                Rgb::BLACK
            } else {
                hue * (target_v / m)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-device runtime: smoothing, rate limits and packet planning
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DeviceRuntime {
    smoothed: Vec<Rgb>,
    /// What we last told the device, i.e. what it is showing.
    pub shown: Vec<[u8; 3]>,
    last_send: f64,
    last_full: f64,
    razer_on_at: Option<f64>,
    last_brightness: Option<u8>,
    last_brightness_t: f64,
    last_kelvin: Option<u32>,
    last_kelvin_t: f64,
    onoff: Option<bool>,
    last_toggle: f64,
    pub msgs_sent: u64,
    pub bytes_sent: u64,
    sent_mode: Option<OutputMode>,
    hue_cache: [u8; 3],
}

impl Default for DeviceRuntime {
    fn default() -> Self {
        // Timestamps start in the distant past so the first tick always sends.
        let past = f64::NEG_INFINITY;
        Self {
            smoothed: Vec::new(),
            shown: Vec::new(),
            last_send: past,
            last_full: past,
            razer_on_at: None,
            last_brightness: None,
            last_brightness_t: past,
            last_kelvin: None,
            last_kelvin_t: past,
            onoff: None,
            last_toggle: past,
            msgs_sent: 0,
            bytes_sent: 0,
            sent_mode: None,
            hue_cache: [255, 255, 255],
        }
    }
}

/// How often unchanged state is re-sent, because UDP packets do get lost.
const REFRESH_S: f64 = 2.5;
/// Razer mode times out after a minute without frames; re-arm well before.
const RAZER_REARM_S: f64 = 20.0;
/// Segment packets bundled per ptReal message.
const PT_PER_MSG: usize = 6;

impl DeviceRuntime {
    /// Called when output stops; returns any "let go" messages.
    pub fn release(&mut self, cfg: &DeviceConfig) -> Vec<Value> {
        let mut out = Vec::new();
        if matches!(cfg.mode, OutputMode::Stream { .. }) && self.razer_on_at.is_some() {
            out.push(govee::cmd_razer(&govee::razer_enable(false)));
        }
        *self = DeviceRuntime {
            msgs_sent: self.msgs_sent,
            bytes_sent: self.bytes_sent,
            ..Default::default()
        };
        out
    }

    /// Advance one engine tick. `px` are the device's target pixels already
    /// sampled from the canvas. Returns JSON messages to send now.
    pub fn plan(
        &mut self,
        cfg: &DeviceConfig,
        px: &[Rgb],
        master: f32,
        now: f64,
        dt: f32,
    ) -> Vec<Value> {
        let n = cfg.mode.pixel_count();
        if self.sent_mode.as_ref() != Some(&cfg.mode) {
            // Mode changed: forget everything we think the device shows.
            let keep = (self.msgs_sent, self.bytes_sent);
            *self = DeviceRuntime {
                msgs_sent: keep.0,
                bytes_sent: keep.1,
                ..Default::default()
            };
            self.sent_mode = Some(cfg.mode.clone());
        }
        // Target colours: master and trim, gamma, then temporal smoothing.
        let gain = master * cfg.trim;
        let gamma = cfg.gamma.clamp(0.3, 3.0);
        let mut target: Vec<Rgb> = px
            .iter()
            .map(|&c| {
                let c = (c * gain).clamp();
                if (gamma - 1.0).abs() > 1e-3 {
                    Rgb::new(c.r.powf(gamma), c.g.powf(gamma), c.b.powf(gamma))
                } else {
                    c
                }
            })
            .collect();
        if target.len() != n {
            target.resize(n, Rgb::BLACK);
        }
        if self.smoothed.len() != n {
            self.smoothed = target.clone();
        }
        let k = 1.0 - cfg.smoothing.clamp(0.0, 0.98).powf(dt * 60.0);
        for (s, t) in self.smoothed.iter_mut().zip(&target) {
            *s = s.lerp(*t, k);
        }

        let interval = 1.0 / cfg.rate_hz.clamp(0.2, 60.0) as f64;
        if now - self.last_send < interval {
            return Vec::new();
        }
        let full = now - self.last_full >= REFRESH_S || self.shown.len() != n;
        let msgs = match cfg.mode.clone() {
            OutputMode::Stream { gradient, .. } => self.plan_stream(gradient, now, full),
            OutputMode::Segments { .. } => self.plan_segments(cfg, full),
            OutputMode::Rgb => self.plan_rgb(cfg, now, full),
            OutputMode::White => self.plan_white(cfg, now, full),
            OutputMode::OnOff => self.plan_onoff(cfg, now, full),
        };
        if !msgs.is_empty() {
            self.last_send = now;
            if full {
                self.last_full = now;
            }
            self.msgs_sent += msgs.len() as u64;
            self.bytes_sent += msgs.iter().map(|m| m.to_string().len() as u64).sum::<u64>();
        }
        msgs
    }

    fn quantised(&self) -> Vec<[u8; 3]> {
        self.smoothed.iter().map(|c| c.to_u8()).collect()
    }

    fn plan_stream(&mut self, gradient: bool, now: f64, full: bool) -> Vec<Value> {
        let mut out = Vec::new();
        let rearm = self.razer_on_at.is_none_or(|t| now - t > RAZER_REARM_S);
        if rearm {
            out.push(govee::cmd_razer(&govee::razer_enable(true)));
            self.razer_on_at = Some(now);
        }
        let q = self.quantised();
        let changed = full
            || rearm
            || q.iter()
                .zip(&self.shown)
                .any(|(a, b)| Rgb::dist2_u8(*a, *b) > 4);
        if changed {
            out.push(govee::cmd_razer(&govee::razer_frame(&q, gradient)));
            self.shown = q;
        }
        out
    }

    fn plan_segments(&mut self, cfg: &DeviceConfig, full: bool) -> Vec<Value> {
        let q = self.quantised();
        // Pick segments whose colour moved enough (or all on a refresh), then
        // group identical colours into one packet using the segment bitmask.
        // groups: (colour, segments, biggest change in the group)
        let mut groups: Vec<([u8; 3], Vec<usize>, i32)> = Vec::new();
        for (i, c) in q.iter().enumerate() {
            let change = self
                .shown
                .get(i)
                .map_or(i32::MAX, |o| Rgb::dist2_u8(*c, *o));
            if !(full || change > 36) {
                continue;
            }
            // Coarser buckets mean more grouping, fewer packets.
            let key = [c[0] & 0xFC, c[1] & 0xFC, c[2] & 0xFC];
            match groups.iter_mut().find(|g| g.0 == key) {
                Some(g) => {
                    g.1.push(i);
                    g.2 = g.2.max(change);
                }
                None => groups.push((key, vec![i], change)),
            }
        }
        // Packet budget: send the most visible changes first; the rest stay
        // "dirty" and go out on the next update.
        let budget = ((cfg.packets_per_sec / cfg.rate_hz.max(0.1)).floor() as usize).max(1);
        if groups.len() > budget {
            groups.sort_by_key(|g| std::cmp::Reverse(g.2));
            groups.truncate(budget);
        }
        if groups.is_empty() {
            return Vec::new();
        }
        if self.shown.len() != q.len() {
            self.shown = vec![[0; 3]; q.len()];
        }
        let packets: Vec<[u8; 20]> = groups
            .iter()
            .map(|(c, segs, _)| {
                for &s in segs {
                    self.shown[s] = *c;
                }
                govee::segment_color_packet(*c, govee::segment_mask(segs.iter().copied()))
            })
            .collect();
        packets
            .chunks(PT_PER_MSG)
            .map(|ch| {
                let refs: Vec<&[u8]> = ch.iter().map(|p| p.as_slice()).collect();
                govee::cmd_pt_real(&refs)
            })
            .collect()
    }

    fn plan_rgb(&mut self, cfg: &DeviceConfig, now: f64, full: bool) -> Vec<Value> {
        let c = self.smoothed.first().copied().unwrap_or_default();
        let mut out = Vec::new();
        match cfg.dim {
            DimVia::Rgb => {
                let q = c.to_u8();
                if full || self.shown.first().is_none_or(|o| Rgb::dist2_u8(q, *o) > 9) {
                    out.push(govee::cmd_color(q));
                    self.shown = vec![q];
                }
            }
            DimVia::Brightness => {
                let m = c.max_channel();
                let hue = if m > 1e-3 {
                    (c * (1.0 / m)).to_u8()
                } else {
                    self.shown_hue()
                };
                let pct = ((m * 100.0).round() as u8).clamp(1, 100);
                let shown_hue = self.shown_hue();
                if full || Rgb::dist2_u8(hue, shown_hue) > 9 {
                    out.push(govee::cmd_color(hue));
                }
                // The brightness command is slow on most bulbs: cap it at 4 Hz.
                let b_due = now - self.last_brightness_t > 0.25;
                if full || (b_due && self.last_brightness.is_none_or(|b| b.abs_diff(pct) >= 2)) {
                    out.push(govee::cmd_brightness(pct));
                    self.last_brightness = Some(pct);
                    self.last_brightness_t = now;
                }
                let shown = (Rgb::from_u8(hue[0], hue[1], hue[2])
                    * (self.last_brightness.unwrap_or(100) as f32 / 100.0))
                    .to_u8();
                self.shown = vec![shown];
                self.hue_cache = hue;
            }
        }
        out
    }

    fn shown_hue(&self) -> [u8; 3] {
        self.hue_cache
    }

    fn plan_white(&mut self, cfg: &DeviceConfig, now: f64, full: bool) -> Vec<Value> {
        let c = self.smoothed.first().copied().unwrap_or_default();
        let (lo, hi) = (
            cfg.min_kelvin.min(cfg.max_kelvin),
            cfg.min_kelvin.max(cfg.max_kelvin),
        );
        let k = (c.approx_kelvin(lo, hi) / 50.0).round() as u32 * 50;
        let pct = ((c.luma().max(c.max_channel() * 0.6) * 100.0).round() as u8).clamp(1, 100);
        let mut out = Vec::new();
        if full
            || (now - self.last_kelvin_t > 0.5
                && self.last_kelvin.is_none_or(|o| o.abs_diff(k) >= 150))
        {
            out.push(govee::cmd_kelvin(k));
            self.last_kelvin = Some(k);
            self.last_kelvin_t = now;
        }
        if full
            || (now - self.last_brightness_t > 0.25
                && self.last_brightness.is_none_or(|b| b.abs_diff(pct) >= 2))
        {
            out.push(govee::cmd_brightness(pct));
            self.last_brightness = Some(pct);
            self.last_brightness_t = now;
        }
        let shown = Rgb::from_kelvin(self.last_kelvin.unwrap_or(4000) as f32)
            * (self.last_brightness.unwrap_or(100) as f32 / 100.0);
        self.shown = vec![shown.to_u8()];
        out
    }

    fn plan_onoff(&mut self, cfg: &DeviceConfig, now: f64, full: bool) -> Vec<Value> {
        let v = self
            .smoothed
            .first()
            .copied()
            .unwrap_or_default()
            .max_channel();
        let th = cfg.onoff_threshold;
        let want = match self.onoff {
            Some(true) => v > th - 0.05,
            Some(false) => v > th + 0.05,
            None => v > th,
        };
        let mut out = Vec::new();
        // Relays and plugs dislike rapid switching: at most once a second.
        let can_toggle = now - self.last_toggle >= 1.0;
        if (Some(want) != self.onoff && can_toggle) || full {
            let state = if Some(want) != self.onoff && can_toggle {
                want
            } else {
                self.onoff.unwrap_or(want)
            };
            out.push(govee::cmd_turn(state));
            if Some(state) != self.onoff {
                self.last_toggle = now;
            }
            self.onoff = Some(state);
        }
        self.shown = vec![if self.onoff == Some(true) {
            [255, 236, 200]
        } else {
            [0, 0, 0]
        }];
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: OutputMode) -> DeviceConfig {
        DeviceConfig {
            rate_hz: 1000.0,
            smoothing: 0.0,
            packets_per_sec: 1e6,
            mode,
            ..Default::default()
        }
    }

    #[test]
    fn span_sampling_box_filters() {
        let canvas: Vec<Rgb> = (0..10)
            .map(|i| if i < 5 { Rgb::WHITE } else { Rgb::BLACK })
            .collect();
        let s = sample_span(&canvas, 0.0, 1.0, 2, false);
        assert_eq!(s[0], Rgb::WHITE);
        assert_eq!(s[1], Rgb::BLACK);
        let r = sample_span(&canvas, 0.0, 1.0, 2, true);
        assert_eq!(r[0], Rgb::BLACK);
        let one = sample_span(&canvas, 0.0, 1.0, 1, false);
        assert!((one[0].r - 0.5).abs() < 1e-5);
        // Upsampling a 1-pixel canvas just repeats it.
        assert_eq!(
            sample_span(&[Rgb::WHITE], 0.0, 1.0, 4, false),
            vec![Rgb::WHITE; 4]
        );
    }

    #[test]
    fn dominant_keeps_a_lone_comet_visible() {
        let mut px = vec![Rgb::BLACK; 30];
        px[10] = Rgb::new(0.0, 0.0, 1.0);
        let avg = collapse(&px, SingleSample::Average);
        let dom = collapse(&px, SingleSample::Dominant);
        assert!(avg.b < 0.05);
        assert!(dom.b > 0.45 && dom.r < 1e-3);
    }

    #[test]
    fn segments_group_identical_colours_and_skip_unchanged() {
        let c = cfg(OutputMode::Segments { count: 10 });
        let mut rt = DeviceRuntime::default();
        let px: Vec<Rgb> = (0..10)
            .map(|i| {
                if i % 2 == 0 {
                    Rgb::new(1.0, 0.0, 0.0)
                } else {
                    Rgb::new(0.0, 0.0, 1.0)
                }
            })
            .collect();
        let m = rt.plan(&c, &px, 1.0, 0.0, 0.016);
        // Two colours -> two packets in one ptReal message.
        assert_eq!(m.len(), 1);
        assert_eq!(m[0]["msg"]["data"]["command"].as_array().unwrap().len(), 2);
        // Nothing changed -> nothing sent until the refresh interval.
        assert!(rt.plan(&c, &px, 1.0, 0.1, 0.016).is_empty());
        assert_eq!(rt.plan(&c, &px, 1.0, REFRESH_S + 0.2, 0.016).len(), 1);
        // One segment changes -> one packet.
        let mut px2 = px.clone();
        px2[3] = Rgb::new(0.0, 1.0, 0.0);
        let m = rt.plan(&c, &px2, 1.0, REFRESH_S + 0.3, 0.016);
        assert_eq!(m[0]["msg"]["data"]["command"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn segment_packet_budget_prioritises_big_changes() {
        let mut c = cfg(OutputMode::Segments { count: 10 });
        c.rate_hz = 8.0;
        c.packets_per_sec = 24.0; // 3 packets per update
        let mut rt = DeviceRuntime::default();
        let px: Vec<Rgb> = (0..10)
            .map(|i| Rgb::hsv(i as f32 / 10.0, 1.0, 1.0))
            .collect();
        let m = rt.plan(&c, &px, 1.0, 0.0, 0.016);
        assert_eq!(m[0]["msg"]["data"]["command"].as_array().unwrap().len(), 3);
        // The remaining segments follow on later updates.
        let m = rt.plan(&c, &px, 1.0, 0.2, 0.016);
        assert_eq!(m[0]["msg"]["data"]["command"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn rate_limit_holds() {
        let mut c = cfg(OutputMode::Rgb);
        c.rate_hz = 10.0;
        let mut rt = DeviceRuntime::default();
        let mut sent = 0;
        for i in 0..60 {
            let t = i as f64 / 60.0;
            let px = [Rgb::hsv(t as f32, 1.0, 1.0)];
            sent += rt.plan(&c, &px, 1.0, t, 1.0 / 60.0).len();
        }
        assert!((9..=11).contains(&sent), "sent {sent}");
    }

    #[test]
    fn stream_arms_razer_first() {
        let c = cfg(OutputMode::Stream {
            leds: 4,
            gradient: false,
        });
        let mut rt = DeviceRuntime::default();
        let m = rt.plan(&c, &[Rgb::WHITE; 4], 1.0, 0.0, 0.016);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0]["msg"]["cmd"], "razer");
        assert_eq!(rt.release(&c).len(), 1);
    }

    #[test]
    fn white_and_onoff_degrade() {
        let c = cfg(OutputMode::White);
        let mut rt = DeviceRuntime::default();
        let m = rt.plan(&c, &[Rgb::new(1.0, 0.4, 0.0)], 1.0, 0.0, 0.016);
        let k = m[0]["msg"]["data"]["colorTemInKelvin"].as_u64().unwrap();
        assert!(k < 4000, "fire-ish colour should map warm, got {k}");

        let c = cfg(OutputMode::OnOff);
        let mut rt = DeviceRuntime::default();
        assert_eq!(
            rt.plan(&c, &[Rgb::WHITE], 1.0, 0.0, 0.016)[0]["msg"]["data"]["value"],
            1
        );
        // Too soon to switch off again.
        assert!(rt.plan(&c, &[Rgb::BLACK], 1.0, 0.5, 0.016).is_empty());
        assert_eq!(
            rt.plan(&c, &[Rgb::BLACK], 1.0, 1.1, 0.016)[0]["msg"]["data"]["value"],
            0
        );
    }

    #[test]
    fn guesses_from_sku_table() {
        assert!(matches!(
            OutputMode::guess("H6199"),
            OutputMode::Segments { .. } | OutputMode::Rgb
        ));
        assert_eq!(OutputMode::guess("NOPE123"), OutputMode::Rgb);
    }
}
