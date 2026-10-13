//! The effect engine: layers of effects rendered onto a virtual LED strip
//! (the "canvas") and blended together. Devices sample the canvas later, so
//! effects never need to know what hardware they end up on.

use crate::audio::{AudioBand, AudioFeatures};
use crate::color::{BlendMode, Rgb};
use crate::noise::{Rng, fbm2, noise1, noise2};
use crate::palette::Palette;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f32::consts::TAU;

// ---------------------------------------------------------------------------
// Effect kinds and their parameters
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectKind {
    Solid,
    Gradient,
    Rainbow,
    Breathe,
    Comet,
    Sparkle,
    Fire,
    Plasma,
    Aurora,
    Candle,
    Strobe,
    Police,
    LavaLamp,
    Ripple,
    Theater,
    Heartbeat,
    Wipe,
    NoiseFlow,
    Spectrum,
    VuMeter,
    BeatPulse,
    BassGlow,
    BeatComets,
}

pub struct KindInfo {
    pub kind: EffectKind,
    pub name: &'static str,
    pub group: &'static str,
    pub blurb: &'static str,
    pub params: &'static [P],
    /// Black pixels let lower layers show through by default.
    pub sparse: bool,
}

/// Which `Params` fields an effect uses. The editor builds its controls from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum P {
    Color,
    Color2,
    Palette,
    Speed,
    Scale,
    Size,
    Density,
    Decay,
    Intensity,
    Count,
    Bounce,
    Mirror,
    Band,
    Gain,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub color: Rgb,
    pub color2: Rgb,
    pub palette: Palette,
    /// Cycles per second (or per beat when the layer is tempo-synced).
    pub speed: f32,
    /// How many times the pattern repeats along the strip.
    pub scale: f32,
    /// Generic "length" (tail, duty cycle, blob size...), 0..1.
    pub size: f32,
    pub density: f32,
    pub decay: f32,
    pub intensity: f32,
    pub count: u32,
    pub bounce: bool,
    pub mirror: bool,
    pub band: AudioBand,
    pub gain: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            color: Rgb::from_hex(0xff3c00),
            color2: Rgb::from_hex(0x0050ff),
            palette: Palette::default(),
            speed: 0.25,
            scale: 1.0,
            size: 0.3,
            density: 0.3,
            decay: 0.5,
            intensity: 0.8,
            count: 3,
            bounce: false,
            mirror: false,
            band: AudioBand::Bass,
            gain: 1.0,
        }
    }
}

pub struct ParamRange {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub log: bool,
    pub help: &'static str,
}

pub fn param_range(p: P) -> ParamRange {
    let r = |label, min, max, log, help| ParamRange {
        label,
        min,
        max,
        log,
        help,
    };
    match p {
        P::Speed => r(
            "Speed",
            0.0,
            8.0,
            true,
            "Cycles per second, or per beat when synced to tempo",
        ),
        P::Scale => r(
            "Scale",
            0.1,
            8.0,
            true,
            "How many times the pattern repeats across the canvas",
        ),
        P::Size => r(
            "Size",
            0.0,
            1.0,
            false,
            "Tail length / duty cycle / blob size",
        ),
        P::Density => r("Density", 0.0, 1.0, false, "How busy it is"),
        P::Decay => r("Decay", 0.0, 1.0, false, "How quickly things fade"),
        P::Intensity => r("Intensity", 0.0, 1.0, false, "Strength of the effect"),
        P::Gain => r("Gain", 0.1, 8.0, true, "Audio sensitivity"),
        _ => r("", 0.0, 1.0, false, ""),
    }
}

use P::*;

pub const KINDS: &[KindInfo] = &[
    KindInfo {
        kind: EffectKind::Solid,
        name: "Solid",
        group: "Basics",
        blurb: "One colour everywhere.",
        params: &[Color],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Gradient,
        name: "Gradient",
        group: "Basics",
        blurb: "Scrolls a palette along the canvas.",
        params: &[Palette, Speed, Scale],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Rainbow,
        name: "Rainbow",
        group: "Basics",
        blurb: "Classic hue sweep.",
        params: &[Speed, Scale, Intensity],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Breathe,
        name: "Breathe",
        group: "Basics",
        blurb: "Slow fade in and out, changing colour each breath.",
        params: &[Palette, Speed, Size],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Wipe,
        name: "Colour wipe",
        group: "Basics",
        blurb: "Each palette colour sweeps over the last.",
        params: &[Palette, Speed, Count],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Comet,
        name: "Comets",
        group: "Motion",
        blurb: "Bright heads with fading tails. Turn on bounce for a scanner.",
        params: &[Palette, Speed, Size, Count, Bounce],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::Theater,
        name: "Theatre chase",
        group: "Motion",
        blurb: "Marquee-style marching lights.",
        params: &[Palette, Speed, Count, Scale],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::Ripple,
        name: "Ripples",
        group: "Motion",
        blurb: "Rings spreading out from random points.",
        params: &[Palette, Speed, Density, Decay],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::Sparkle,
        name: "Sparkle",
        group: "Motion",
        blurb: "Random twinkles that fade out.",
        params: &[Palette, Density, Decay],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::Strobe,
        name: "Strobe",
        group: "Motion",
        blurb: "Flashes. Sync to tempo for beat-locked strobing.",
        params: &[Color, Speed, Size],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::Police,
        name: "Police",
        group: "Motion",
        blurb: "Alternating double-flash between two colours.",
        params: &[Color, Color2, Speed],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Heartbeat,
        name: "Heartbeat",
        group: "Motion",
        blurb: "Lub-dub pulse. Lovely when tempo-synced.",
        params: &[Color, Speed, Decay],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::Fire,
        name: "Fire",
        group: "Nature",
        blurb: "Flickering flames rising from the start of the canvas.",
        params: &[Palette, Speed, Density, Decay, Mirror],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Candle,
        name: "Candle",
        group: "Nature",
        blurb: "Gentle warm flicker. Works on single-colour bulbs too.",
        params: &[Color, Intensity, Speed],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Aurora,
        name: "Aurora",
        group: "Nature",
        blurb: "Drifting curtains of light.",
        params: &[Palette, Speed, Scale, Intensity],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Plasma,
        name: "Plasma",
        group: "Nature",
        blurb: "Interfering sine waves through a palette.",
        params: &[Palette, Speed, Scale],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::LavaLamp,
        name: "Lava lamp",
        group: "Nature",
        blurb: "Gooey blobs merging and splitting.",
        params: &[Palette, Speed, Count, Size],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::NoiseFlow,
        name: "Noise flow",
        group: "Nature",
        blurb: "Organic palette drift. A good background.",
        params: &[Palette, Speed, Scale],
        sparse: false,
    },
    KindInfo {
        kind: EffectKind::Spectrum,
        name: "Spectrum",
        group: "Audio",
        blurb: "Frequency bands laid out along the canvas.",
        params: &[Palette, Gain, Mirror],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::VuMeter,
        name: "VU meter",
        group: "Audio",
        blurb: "Level meter that fills the canvas.",
        params: &[Palette, Band, Gain, Mirror],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::BeatPulse,
        name: "Beat pulse",
        group: "Audio",
        blurb: "Flashes on each beat and steps through the palette.",
        params: &[Palette, Decay],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::BassGlow,
        name: "Band glow",
        group: "Audio",
        blurb: "Brightness follows one frequency band.",
        params: &[Palette, Band, Gain],
        sparse: true,
    },
    KindInfo {
        kind: EffectKind::BeatComets,
        name: "Beat comets",
        group: "Audio",
        blurb: "Each beat fires a comet down the canvas.",
        params: &[Palette, Speed, Size, Mirror],
        sparse: true,
    },
];

pub fn info(kind: EffectKind) -> &'static KindInfo {
    KINDS
        .iter()
        .find(|k| k.kind == kind)
        .expect("every kind is listed")
}

// ---------------------------------------------------------------------------
// Modulation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ModSource {
    #[default]
    Off,
    /// Free-running oscillator, rate in Hz.
    Lfo,
    /// Oscillator locked to the tempo clock, rate in cycles per beat.
    Tempo,
    /// An audio band's level.
    Audio,
    /// Smooth random wandering.
    Wander,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ModShape {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ModTarget {
    #[default]
    Opacity,
    Speed,
    Hue,
    Scale,
    Offset,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Modulator {
    pub source: ModSource,
    pub shape: ModShape,
    pub target: ModTarget,
    pub rate: f32,
    pub depth: f32,
    pub band: AudioBand,
}

impl Default for Modulator {
    fn default() -> Self {
        Self {
            source: ModSource::Off,
            shape: ModShape::Sine,
            target: ModTarget::Opacity,
            rate: 0.25,
            depth: 0.5,
            band: AudioBand::Bass,
        }
    }
}

impl ModShape {
    fn eval(self, phase: f32) -> f32 {
        let p = phase.rem_euclid(1.0);
        match self {
            ModShape::Sine => 0.5 - 0.5 * (p * TAU).cos(),
            ModShape::Triangle => 1.0 - (2.0 * p - 1.0).abs(),
            ModShape::Saw => p,
            ModShape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layers and programs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub kind: EffectKind,
    pub params: Params,
    pub blend: BlendMode,
    pub opacity: f32,
    /// Part of the canvas this layer covers, as fractions.
    pub start: f32,
    pub end: f32,
    pub reverse: bool,
    pub hue_shift: f32,
    /// Treat dark pixels as transparent so lower layers show through.
    pub see_through: bool,
    /// Run this layer's clock in beats instead of seconds.
    pub tempo_sync: bool,
    pub modulator: Modulator,
}

impl Default for Layer {
    fn default() -> Self {
        Self::new(EffectKind::Gradient)
    }
}

impl Layer {
    pub fn new(kind: EffectKind) -> Self {
        let i = info(kind);
        let mut params = Params::default();
        // A few per-kind defaults so new layers look good straight away.
        match kind {
            EffectKind::Fire => {
                params.palette = crate::palette::builtin("Fire").unwrap();
                params.speed = 1.0;
                params.density = 0.5;
                params.decay = 0.45;
            }
            EffectKind::Aurora => params.palette = crate::palette::builtin("Aurora").unwrap(),
            EffectKind::LavaLamp => {
                params.palette = crate::palette::builtin("Lava").unwrap();
                params.speed = 0.1;
            }
            EffectKind::Candle => {
                params.color = Rgb::from_hex(0xff8a20);
                params.speed = 1.0;
            }
            EffectKind::Strobe => {
                params.color = Rgb::WHITE;
                params.speed = 4.0;
                params.size = 0.15;
            }
            EffectKind::Police => {
                params.color = Rgb::from_hex(0xff0000);
                params.color2 = Rgb::from_hex(0x0000ff);
                params.speed = 1.0;
            }
            EffectKind::Heartbeat => {
                params.color = Rgb::from_hex(0xff0030);
                params.speed = 1.1;
            }
            EffectKind::Comet => params.speed = 0.4,
            EffectKind::Rainbow => params.intensity = 1.0,
            EffectKind::Sparkle => params.palette = crate::palette::builtin("Ice").unwrap(),
            EffectKind::Spectrum | EffectKind::VuMeter => {
                params.palette = Palette::from_hex(&[0x00ff40, 0xffee00, 0xff0000], false)
            }
            EffectKind::BeatComets => params.speed = 1.2,
            _ => {}
        }
        Self {
            id: next_id(),
            name: i.name.to_string(),
            enabled: true,
            kind,
            params,
            blend: if i.sparse {
                BlendMode::Screen
            } else {
                BlendMode::Normal
            },
            opacity: 1.0,
            start: 0.0,
            end: 1.0,
            reverse: false,
            hue_shift: 0.0,
            see_through: i.sparse,
            tempo_sync: false,
            modulator: Modulator::default(),
        }
    }
}

pub fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    // Seeded from the clock so ids from different sessions don't collide in saved presets.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(1);
    let _ = NEXT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Program {
    pub name: String,
    /// Bottom layer first.
    pub layers: Vec<Layer>,
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Everything an effect may read while rendering one frame.
pub struct Frame<'a> {
    pub dt: f32,
    /// Global speed multiplier.
    pub speed: f32,
    pub bpm: f32,
    /// Beats elapsed since start (fractional).
    pub beats: f64,
    pub audio: &'a AudioFeatures,
}

#[derive(Default)]
struct LayerState {
    kind: Option<EffectKind>,
    len: usize,
    phase: f64,
    mod_phase: f64,
    rng: Option<Rng>,
    heat: Vec<f32>,
    px: Vec<Rgb>,
    ripples: Vec<(f32, f32, Rgb)>, // centre, age, colour
    comets: Vec<(f32, Rgb)>,       // position, colour
    last_beat: u64,
    level: f32,
    spawn_acc: f32,
}

#[derive(Default)]
pub struct Renderer {
    states: HashMap<u64, LayerState>,
    scratch: Vec<Rgb>,
}

impl Renderer {
    pub fn render(&mut self, program: &Program, f: &Frame, out: &mut [Rgb]) {
        out.fill(Rgb::BLACK);
        let n = out.len();
        if n == 0 {
            return;
        }
        let live: Vec<u64> = program.layers.iter().map(|l| l.id).collect();
        self.states.retain(|id, _| live.contains(id));

        for layer in program
            .layers
            .iter()
            .filter(|l| l.enabled && l.opacity > 0.0)
        {
            let (a, b) = (layer.start.min(layer.end), layer.start.max(layer.end));
            let i0 = ((a * n as f32).round() as usize).min(n);
            let i1 = ((b * n as f32).round() as usize).clamp(i0, n);
            let len = i1 - i0;
            if len == 0 {
                continue;
            }
            let st = self.states.entry(layer.id).or_default();
            if st.kind != Some(layer.kind) || st.len != len {
                *st = LayerState {
                    kind: Some(layer.kind),
                    len,
                    rng: Some(Rng::new(layer.id)),
                    ..Default::default()
                };
            }

            // Modulation.
            let m = &layer.modulator;
            let clock_rate = if layer.tempo_sync { f.bpm / 60.0 } else { 1.0 };
            let mv = match m.source {
                ModSource::Off => None,
                ModSource::Lfo => {
                    st.mod_phase += (f.dt * m.rate) as f64;
                    Some(m.shape.eval(st.mod_phase as f32))
                }
                ModSource::Tempo => Some(m.shape.eval((f.beats * m.rate.max(0.01) as f64) as f32)),
                ModSource::Audio => Some((f.audio.band(m.band) * 1.2).min(1.0)),
                ModSource::Wander => {
                    st.mod_phase += (f.dt * m.rate) as f64;
                    Some(noise1(st.mod_phase as f32 * 3.0))
                }
            };
            let modv = |target: ModTarget| -> f32 {
                match mv {
                    Some(v) if m.target == target => v,
                    _ => -1.0,
                }
            };
            let mut opacity = layer.opacity;
            if modv(ModTarget::Opacity) >= 0.0 {
                opacity *= 1.0 - m.depth + m.depth * modv(ModTarget::Opacity);
            }
            let mut speed_mul = 1.0;
            if modv(ModTarget::Speed) >= 0.0 {
                speed_mul = 1.0 + m.depth * 3.0 * (modv(ModTarget::Speed) - 0.5) * 2.0;
                speed_mul = speed_mul.max(0.0);
            }
            let scaled;
            let params = if modv(ModTarget::Scale) >= 0.0 {
                let mut p = layer.params.clone();
                p.scale *= 1.0 + m.depth * 2.0 * (modv(ModTarget::Scale) - 0.5);
                scaled = p;
                &scaled
            } else {
                &layer.params
            };
            let hue = layer.hue_shift
                + if modv(ModTarget::Hue) >= 0.0 {
                    m.depth * modv(ModTarget::Hue)
                } else {
                    0.0
                };
            let offset = if modv(ModTarget::Offset) >= 0.0 {
                m.depth * modv(ModTarget::Offset)
            } else {
                0.0
            };

            let dt_eff = f.dt * f.speed * clock_rate * speed_mul;
            st.phase += (dt_eff * params.speed) as f64;

            self.scratch.clear();
            self.scratch.resize(len, Rgb::BLACK);
            render_effect(layer.kind, params, st, f, &mut self.scratch);

            if offset > 0.0 {
                let shift = (offset * len as f32) as usize % len;
                self.scratch.rotate_right(shift);
            }
            if layer.reverse {
                self.scratch.reverse();
            }

            for (k, &px) in self.scratch.iter().enumerate() {
                let mut px = px;
                if hue.abs() > 1e-4 {
                    let (h, s, v) = px.to_hsv();
                    px = Rgb::hsv(h + hue, s, v);
                }
                let (top, alpha) = if layer.see_through {
                    let m = px.max_channel();
                    if m <= 1e-4 {
                        (Rgb::BLACK, 0.0)
                    } else {
                        (px * (1.0 / m), m.min(1.0))
                    }
                } else {
                    (px, 1.0)
                };
                let dst = &mut out[i0 + k];
                *dst = layer.blend.apply(*dst, top, alpha * opacity);
            }
        }
    }
}

fn tri(x: f32) -> f32 {
    1.0 - (2.0 * x.rem_euclid(1.0) - 1.0).abs()
}

/// Mirror helper: render into the first half and reflect it.
#[allow(clippy::needless_range_loop)]
fn mirrored(out: &mut [Rgb], mirror: bool, mut draw: impl FnMut(&mut [Rgb])) {
    if !mirror || out.len() < 2 {
        draw(out);
        return;
    }
    let n = out.len();
    let half = n.div_ceil(2);
    let mut tmp = vec![Rgb::BLACK; half];
    draw(&mut tmp);
    for i in 0..n {
        // Centre-out: index 0 of the half is the middle of the strip.
        let d = (i as isize - (n as isize - 1) / 2)
            .unsigned_abs()
            .min(half - 1);
        out[i] = tmp[d];
    }
}

fn render_effect(kind: EffectKind, p: &Params, st: &mut LayerState, f: &Frame, out: &mut [Rgb]) {
    let n = out.len();
    let nf = n as f32;
    let ph = st.phase as f32;
    let rng = st.rng.get_or_insert_with(|| Rng::new(7));
    let x_of = |i: usize| if n > 1 { i as f32 / (nf - 1.0) } else { 0.5 };

    match kind {
        EffectKind::Solid => out.fill(p.color),

        EffectKind::Gradient => {
            for (i, o) in out.iter_mut().enumerate() {
                *o = p.palette.sample(x_of(i) * p.scale - ph);
            }
        }

        EffectKind::Rainbow => {
            for (i, o) in out.iter_mut().enumerate() {
                *o = Rgb::hsv(x_of(i) * p.scale - ph, p.intensity, 1.0);
            }
        }

        EffectKind::Breathe => {
            let env = (0.5 - 0.5 * (ph * TAU).cos()).powf(1.6);
            let lvl = p.size * 0.6 + (1.0 - p.size * 0.6) * env;
            let c = p.palette.sample(ph.floor() * 0.618_034);
            out.fill(c * lvl);
        }

        EffectKind::Wipe => {
            let steps = p.count.max(2) as f32;
            let t = ph;
            let idx = t.floor();
            let frac = t.fract();
            let back = p.palette.sample(idx / steps);
            let front = p.palette.sample((idx + 1.0) / steps);
            let edge = frac * nf;
            for (i, o) in out.iter_mut().enumerate() {
                let k = (edge - i as f32).clamp(0.0, 1.0);
                *o = back.lerp(front, k);
            }
        }

        EffectKind::Comet => {
            let count = p.count.max(1);
            let tail = (p.size * 0.9 + 0.02) * nf;
            for c in 0..count {
                let t = ph + c as f32 / count as f32;
                let head = if p.bounce { tri(t * 0.5) } else { t.fract() } * (nf - 1.0);
                let dir = if p.bounce && (t * 0.5).rem_euclid(1.0) >= 0.5 {
                    -1.0
                } else {
                    1.0
                };
                let col = p.palette.sample(c as f32 / count as f32 + ph * 0.05);
                for (i, o) in out.iter_mut().enumerate() {
                    let mut d = (head - i as f32) * dir;
                    if !p.bounce && d < 0.0 {
                        d += nf; // wrap the tail around
                    }
                    if d >= -0.5 && d <= tail {
                        let k = (1.0 - (d.max(0.0) / tail)).powf(2.2);
                        *o = BlendMode::Lighten.apply(*o, col * k, 1.0);
                    }
                }
            }
        }

        EffectKind::Theater => {
            let spacing = p.count.max(2) as usize;
            let step = (ph * spacing as f32).floor() as isize;
            for (i, o) in out.iter_mut().enumerate() {
                if (i as isize + step).rem_euclid(spacing as isize) == 0 {
                    *o = p.palette.sample(x_of(i) * p.scale + ph * 0.1);
                }
            }
        }

        EffectKind::Ripple => {
            st.spawn_acc += f.dt * f.speed * (0.2 + p.density * 3.0);
            while st.spawn_acc >= 1.0 {
                st.spawn_acc -= 1.0;
                let c = p.palette.sample(rng.f32());
                st.ripples.push((rng.f32(), 0.0, c));
            }
            let speed = 0.15 + p.speed * 0.6;
            let life = 0.4 + (1.0 - p.decay) * 2.5;
            for r in st.ripples.iter_mut() {
                r.1 += f.dt * f.speed;
            }
            st.ripples.retain(|r| r.1 < life);
            for (i, o) in out.iter_mut().enumerate() {
                let x = x_of(i);
                for &(cx, age, col) in &st.ripples {
                    let radius = age * speed;
                    let d = ((x - cx).abs() - radius).abs() * nf;
                    if d < 2.5 {
                        let k = (1.0 - d / 2.5) * (1.0 - age / life);
                        *o = BlendMode::Lighten.apply(*o, col * k, 1.0);
                    }
                }
            }
        }

        EffectKind::Sparkle => {
            if st.px.len() != n {
                st.px = vec![Rgb::BLACK; n];
            }
            let fade = (1.0 - (0.4 + p.decay * 6.0) * f.dt * f.speed.max(0.05)).clamp(0.0, 1.0);
            for c in st.px.iter_mut() {
                *c = *c * fade;
            }
            st.spawn_acc += f.dt * f.speed * nf * p.density * p.density * 2.0;
            while st.spawn_acc >= 1.0 {
                st.spawn_acc -= 1.0;
                let i = rng.below(n);
                st.px[i] = p.palette.sample(rng.f32());
            }
            out.copy_from_slice(&st.px);
        }

        EffectKind::Strobe => {
            let on = ph.fract() < p.size.max(0.02);
            out.fill(if on { p.color } else { Rgb::BLACK });
        }

        EffectKind::Police => {
            // Two flashes on side A, two on side B, per cycle.
            let t = ph.fract();
            // Within each half-cycle: flash, gap, flash, long gap.
            let flash = |u: f32| u < 0.15 || (0.25..0.4).contains(&u);
            let a_on = t < 0.5 && flash(t * 2.0);
            let b_on = t >= 0.5 && flash((t - 0.5) * 2.0);
            let half = n / 2;
            for (i, o) in out.iter_mut().enumerate() {
                *o = if i < half.max(1) {
                    if a_on { p.color } else { Rgb::BLACK }
                } else if b_on {
                    p.color2
                } else {
                    Rgb::BLACK
                };
            }
            if n == 1 {
                out[0] = if a_on {
                    p.color
                } else if b_on {
                    p.color2
                } else {
                    Rgb::BLACK
                };
            }
        }

        EffectKind::Heartbeat => {
            let t = ph.fract();
            let w = 0.04 + p.decay * 0.12;
            let pulse = |c: f32| (-((t - c) / w).powi(2)).exp();
            let env = (pulse(0.05) + 0.65 * pulse(0.28)).min(1.0);
            out.fill(p.color * env);
        }

        EffectKind::Fire => {
            mirrored(out, p.mirror, |buf| {
                let m = buf.len();
                if st.heat.len() != m {
                    st.heat = vec![0.0; m];
                }
                // Fire2012, scaled to a fixed 60 Hz simulation step.
                st.spawn_acc += f.dt * 60.0 * p.speed.max(0.05) * f.speed;
                while st.spawn_acc >= 1.0 {
                    st.spawn_acc -= 1.0;
                    let cooling = 0.02 + p.decay * 0.25;
                    for h in st.heat.iter_mut() {
                        *h = (*h - rng.f32() * cooling * 10.0 / m as f32 * 4.0).max(0.0);
                    }
                    for k in (2..m).rev() {
                        st.heat[k] = (st.heat[k - 1] + st.heat[k - 2] * 2.0) / 3.0;
                    }
                    if m >= 2 && rng.f32() < 0.3 + p.density * 0.7 {
                        let y = rng.below((m / 6).max(1));
                        st.heat[y] = (st.heat[y] + rng.range(0.6, 1.0)).min(1.0);
                    }
                }
                for (o, &h) in buf.iter_mut().zip(&st.heat) {
                    *o = p.palette.sample(h * 0.98);
                }
            });
        }

        EffectKind::Candle => {
            let t = ph * 6.0;
            let base =
                1.0 - p.intensity * 0.55 * noise1(t) - p.intensity * 0.25 * noise1(t * 3.7 + 9.0);
            let gutter = if noise1(t * 0.3 + 40.0) > 0.8 {
                0.6
            } else {
                1.0
            };
            for (i, o) in out.iter_mut().enumerate() {
                let local = 1.0 - p.intensity * 0.15 * noise2(i as f32 * 0.3, t * 2.0);
                *o = p.color * (base * local * gutter).clamp(0.0, 1.0);
            }
        }

        EffectKind::Aurora => {
            for (i, o) in out.iter_mut().enumerate() {
                let x = x_of(i) * p.scale * 3.0;
                let curtain = fbm2(x + ph * 0.7, ph * 0.3);
                let glow = fbm2(x * 0.5 - ph * 0.4, 13.0 + ph * 0.2);
                let lvl = (curtain * 1.4 - 0.25)
                    .clamp(0.0, 1.0)
                    .powf(1.0 + p.intensity * 1.5);
                *o = p.palette.sample(glow + ph * 0.05) * (0.1 + 0.9 * lvl);
            }
        }

        EffectKind::Plasma => {
            for (i, o) in out.iter_mut().enumerate() {
                let x = x_of(i) * p.scale * TAU;
                let v = (x + ph * TAU).sin()
                    + (x * 0.73 - ph * 4.1).sin()
                    + ((x * 1.7 + ph * 2.3).sin() * 1.3).sin();
                *o = p.palette.sample(v / 6.0 + 0.5 + ph * 0.1);
            }
        }

        EffectKind::LavaLamp => {
            let count = p.count.clamp(1, 12);
            let r = 0.03 + p.size * 0.15;
            for (i, o) in out.iter_mut().enumerate() {
                let x = x_of(i);
                let mut field = 0.0;
                for b in 0..count {
                    let bf = b as f32;
                    let c = 0.5 + 0.45 * ((ph * (0.7 + bf * 0.31)) * TAU + bf * 2.1).sin();
                    let d = (x - c).abs().max(0.004);
                    field += (r * r) / (d * d);
                }
                let v = (field / (1.0 + field)).clamp(0.0, 1.0);
                *o = p.palette.sample(v * 0.9 + 0.05);
            }
        }

        EffectKind::NoiseFlow => {
            for (i, o) in out.iter_mut().enumerate() {
                let v = fbm2(x_of(i) * p.scale * 2.5, ph);
                *o = p.palette.sample(v * 1.6 + ph * 0.1);
            }
        }

        EffectKind::Spectrum => {
            let bands = &f.audio.spectrum;
            mirrored(out, p.mirror, |buf| {
                let m = buf.len();
                for (i, o) in buf.iter_mut().enumerate() {
                    let pos = if m > 1 {
                        i as f32 / (m - 1) as f32
                    } else {
                        0.0
                    };
                    let fi = pos * (bands.len() - 1) as f32;
                    let lo = fi.floor() as usize;
                    let hi = (lo + 1).min(bands.len() - 1);
                    let v = bands[lo] + (bands[hi] - bands[lo]) * fi.fract();
                    let v = (v * p.gain).clamp(0.0, 1.0);
                    *o = p.palette.sample(pos) * v;
                }
            });
        }

        EffectKind::VuMeter => {
            let target = (f.audio.band(p.band) * p.gain).clamp(0.0, 1.0);
            // Fast attack, slow release, like a real meter.
            st.level = if target > st.level {
                target
            } else {
                st.level + (target - st.level) * (f.dt * 4.0).min(1.0)
            };
            let level = st.level;
            mirrored(out, p.mirror, |buf| {
                let m = buf.len() as f32;
                for (i, o) in buf.iter_mut().enumerate() {
                    let pos = i as f32 / m.max(1.0);
                    let fill = (level * m - i as f32).clamp(0.0, 1.0);
                    *o = p.palette.sample(pos) * fill;
                }
            });
        }

        EffectKind::BeatPulse => {
            if f.audio.beat_count != st.last_beat {
                st.last_beat = f.audio.beat_count;
                st.level = 1.0;
                st.phase = (st.phase + 0.618_034).fract();
            }
            st.level *= (1.0 - (1.5 + p.decay * 10.0) * f.dt).clamp(0.0, 1.0);
            out.fill(p.palette.sample(st.phase as f32) * st.level);
        }

        EffectKind::BassGlow => {
            let v = (f.audio.band(p.band) * p.gain).clamp(0.0, 1.0);
            st.level += (v - st.level) * (f.dt * 12.0).min(1.0);
            out.fill(p.palette.sample(st.level * 0.9) * st.level);
        }

        EffectKind::BeatComets => {
            if f.audio.beat_count != st.last_beat {
                st.last_beat = f.audio.beat_count;
                let c = p.palette.sample(rng.f32());
                st.comets.push((0.0, c));
            }
            let speed = 0.3 + p.speed * 0.8;
            for c in st.comets.iter_mut() {
                c.0 += f.dt * speed * f.speed;
            }
            let tail = 0.03 + p.size * 0.5;
            st.comets.retain(|c| c.0 - tail < 1.0);
            let comets = st.comets.clone();
            mirrored(out, p.mirror, |buf| {
                let m = buf.len();
                for (i, o) in buf.iter_mut().enumerate() {
                    let x = if m > 1 {
                        i as f32 / (m - 1) as f32
                    } else {
                        0.0
                    };
                    for &(head, col) in &comets {
                        let d = head - x;
                        if (0.0..=tail).contains(&d) {
                            let k = (1.0 - d / tail).powf(2.0);
                            *o = BlendMode::Lighten.apply(*o, col * k, 1.0);
                        }
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_renders_finite_values() {
        let audio = AudioFeatures::demo(0.7);
        let mut r = Renderer::default();
        for k in KINDS {
            let prog = Program {
                name: "t".into(),
                layers: vec![Layer::new(k.kind)],
            };
            let mut out = vec![Rgb::BLACK; 60];
            for step in 0..120 {
                let f = Frame {
                    dt: 1.0 / 60.0,
                    speed: 1.0,
                    bpm: 120.0,
                    beats: step as f64 / 30.0,
                    audio: &audio,
                };
                r.render(&prog, &f, &mut out);
            }
            for px in &out {
                assert!(
                    px.r.is_finite() && px.g.is_finite() && px.b.is_finite(),
                    "{}",
                    k.name
                );
                assert!(px.r <= 1.0 && px.r >= 0.0, "{}", k.name);
            }
            // Single-pixel canvases must not panic either.
            let mut one = vec![Rgb::BLACK; 1];
            let f = Frame {
                dt: 0.016,
                speed: 1.0,
                bpm: 120.0,
                beats: 0.0,
                audio: &audio,
            };
            r.render(&prog, &f, &mut one);
        }
    }

    #[test]
    fn ids_are_unique() {
        let ids: std::collections::HashSet<u64> = (0..1000).map(|_| next_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn layer_region_is_respected() {
        let audio = AudioFeatures::default();
        let mut l = Layer::new(EffectKind::Solid);
        l.params.color = Rgb::WHITE;
        l.start = 0.5;
        let prog = Program {
            name: "t".into(),
            layers: vec![l],
        };
        let mut out = vec![Rgb::BLACK; 10];
        let f = Frame {
            dt: 0.016,
            speed: 1.0,
            bpm: 120.0,
            beats: 0.0,
            audio: &audio,
        };
        Renderer::default().render(&prog, &f, &mut out);
        assert_eq!(out[0], Rgb::BLACK);
        assert_eq!(out[9], Rgb::WHITE);
    }
}
