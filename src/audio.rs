//! Audio capture and analysis for the audio-reactive effects.
//!
//! Capture uses cpal (behind the `audio` feature). On Windows the "System
//! audio" source opens the default *output* device as an input, which WASAPI
//! turns into loopback capture: the lights react to whatever the PC plays.

use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub const BANDS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioBand {
    #[default]
    Bass,
    Mid,
    Treble,
    Level,
}

impl AudioBand {
    pub const ALL: [AudioBand; 4] = [
        AudioBand::Bass,
        AudioBand::Mid,
        AudioBand::Treble,
        AudioBand::Level,
    ];
    pub fn label(self) -> &'static str {
        match self {
            AudioBand::Bass => "Bass",
            AudioBand::Mid => "Mids",
            AudioBand::Treble => "Treble",
            AudioBand::Level => "Overall level",
        }
    }
}

/// Normalised (roughly 0..1, auto-gained) features for the current frame.
#[derive(Clone, Debug, Default)]
pub struct AudioFeatures {
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub spectrum: [f32; BANDS],
    /// Increments on every detected beat; effects compare against their last seen value.
    pub beat_count: u64,
    /// 1.0 right after a beat, decaying to 0.
    pub beat_pulse: f32,
    pub active: bool,
}

impl AudioFeatures {
    pub fn band(&self, b: AudioBand) -> f32 {
        match b {
            AudioBand::Bass => self.bass,
            AudioBand::Mid => self.mid,
            AudioBand::Treble => self.treble,
            AudioBand::Level => self.level,
        }
    }

    /// Synthetic features, used by tests and the "demo signal" toggle so
    /// audio effects can be designed without music playing.
    pub fn demo(t: f32) -> Self {
        let beat = (t * 2.0).fract();
        let kick = (-beat * 7.0).exp();
        let mut spectrum = [0.0; BANDS];
        for (i, s) in spectrum.iter_mut().enumerate() {
            let x = i as f32 / BANDS as f32;
            *s = ((1.0 - x) * kick * 0.9
                + 0.25 * (0.5 + 0.5 * (t * 3.0 + i as f32).sin()) * (1.0 - x * 0.5))
                .clamp(0.0, 1.0);
        }
        Self {
            level: 0.4 + 0.4 * kick,
            bass: kick,
            mid: 0.3 + 0.2 * (t * 1.7).sin().abs(),
            treble: 0.2 + 0.3 * (t * 5.3).sin().abs(),
            spectrum,
            beat_count: (t * 2.0) as u64,
            beat_pulse: kick,
            active: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AudioSource {
    #[default]
    Off,
    /// A synthetic 120 BPM signal for designing audio effects in silence.
    Demo,
    /// Default output device via loopback (Windows/WASAPI); falls back to the
    /// default input elsewhere.
    System,
    /// A named input device (microphone, line-in, virtual cable...).
    Input(String),
}

impl AudioSource {
    pub fn label(&self) -> String {
        match self {
            AudioSource::Off => "Off".into(),
            AudioSource::Demo => "Demo signal (120 BPM)".into(),
            AudioSource::System => {
                if cfg!(windows) {
                    "System audio (loopback)".into()
                } else {
                    "Default input".into()
                }
            }
            AudioSource::Input(n) => n.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

const FFT_SIZE: usize = 1024;

pub struct Analyzer {
    fft: Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    peaks: [f32; BANDS],
    smooth: [f32; BANDS],
    bass_avg: f32,
    since_beat: f32,
    flux_prev: f32,
    pub features: AudioFeatures,
}

impl Default for Analyzer {
    fn default() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        Self {
            fft,
            window,
            buf: vec![Complex::default(); FFT_SIZE],
            peaks: [0.05; BANDS],
            smooth: [0.0; BANDS],
            bass_avg: 0.0,
            since_beat: 1.0,
            flux_prev: 0.0,
            features: AudioFeatures::default(),
        }
    }
}

impl Analyzer {
    /// Analyse the most recent `FFT_SIZE` mono samples. `dt` is the time
    /// since the previous call.
    #[allow(clippy::needless_range_loop)]
    pub fn process(&mut self, samples: &[f32], sample_rate: f32, dt: f32) {
        if samples.len() < FFT_SIZE {
            self.decay(dt);
            return;
        }
        let tail = &samples[samples.len() - FFT_SIZE..];
        for ((c, &s), &w) in self.buf.iter_mut().zip(tail).zip(&self.window) {
            *c = Complex::new(s * w, 0.0);
        }
        self.fft.process(&mut self.buf);

        // Log-spaced bands between 40 Hz and 16 kHz.
        let bin_hz = sample_rate / FFT_SIZE as f32;
        let (lo, hi) = (40.0f32, 16000.0f32.min(sample_rate * 0.45));
        let mut raw = [0.0f32; BANDS];
        for (b, r) in raw.iter_mut().enumerate() {
            let f0 = lo * (hi / lo).powf(b as f32 / BANDS as f32);
            let f1 = lo * (hi / lo).powf((b + 1) as f32 / BANDS as f32);
            let i0 = ((f0 / bin_hz) as usize).max(1);
            let i1 = ((f1 / bin_hz) as usize).max(i0 + 1).min(FFT_SIZE / 2);
            let mut e = 0.0;
            for c in &self.buf[i0..i1] {
                e += c.norm();
            }
            *r = (e / (i1 - i0) as f32).sqrt();
        }

        // Auto-gain per band: track a slowly decaying peak.
        let mut flux = 0.0;
        for b in 0..BANDS {
            self.peaks[b] = (self.peaks[b] * (1.0 - 0.15 * dt)).max(raw[b]).max(0.02);
            let v = (raw[b] / self.peaks[b]).clamp(0.0, 1.0);
            let k = if v > self.smooth[b] {
                0.6
            } else {
                (dt * 8.0).min(1.0)
            };
            let prev = self.smooth[b];
            self.smooth[b] += (v - self.smooth[b]) * k;
            if b < 4 {
                flux += (self.smooth[b] - prev).max(0.0);
            }
        }
        let f = &mut self.features;
        f.spectrum = self.smooth;
        let avg = |r: std::ops::Range<usize>| {
            r.clone().map(|i| self.smooth[i]).sum::<f32>() / r.len() as f32
        };
        f.bass = avg(0..4);
        f.mid = avg(4..10);
        f.treble = avg(10..BANDS);
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / FFT_SIZE as f32).sqrt();
        f.level = (rms * 4.0).clamp(0.0, 1.0).max(avg(0..BANDS));
        f.active = rms > 1e-4;

        // Beat detection: bass onset well above its running average.
        self.since_beat += dt;
        self.bass_avg += (f.bass - self.bass_avg) * (dt * 1.5).min(1.0);
        let onset = flux > 0.12 && flux > self.flux_prev * 1.2;
        if f.active && f.bass > self.bass_avg * 1.25 + 0.08 && onset && self.since_beat > 0.22 {
            self.since_beat = 0.0;
            f.beat_count += 1;
            f.beat_pulse = 1.0;
        }
        self.flux_prev = flux;
        f.beat_pulse *= (1.0 - dt * 6.0).max(0.0);
    }

    fn decay(&mut self, dt: f32) {
        let f = &mut self.features;
        let k = (1.0 - dt * 3.0).max(0.0);
        f.level *= k;
        f.bass *= k;
        f.mid *= k;
        f.treble *= k;
        for s in f.spectrum.iter_mut() {
            *s *= k;
        }
        f.beat_pulse *= k;
        f.active = false;
    }
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Shared ring buffer filled by the capture thread.
pub struct Capture {
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub sample_rate: Arc<Mutex<f32>>,
    pub status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Capture {
    pub fn latest(&self, out: &mut Vec<f32>) -> f32 {
        out.clear();
        if let Ok(s) = self.samples.lock() {
            out.extend_from_slice(&s);
        }
        *self.sample_rate.lock().unwrap()
    }
}

pub fn input_device_names() -> Vec<String> {
    #[cfg(feature = "audio")]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        let mut names = Vec::new();
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if let Ok(desc) = d.description() {
                    names.push(desc.name().to_string());
                }
            }
        }
        names.sort();
        names.dedup();
        names
    }
    #[cfg(not(feature = "audio"))]
    {
        Vec::new()
    }
}

pub fn start(source: &AudioSource) -> Option<Capture> {
    if matches!(source, AudioSource::Off | AudioSource::Demo) {
        return None;
    }
    let cap = Capture {
        samples: Arc::new(Mutex::new(Vec::with_capacity(FFT_SIZE * 2))),
        sample_rate: Arc::new(Mutex::new(48000.0)),
        status: Arc::new(Mutex::new("Starting…".into())),
        stop: Arc::new(AtomicBool::new(false)),
    };
    #[cfg(feature = "audio")]
    {
        let (samples, rate, status, stop) = (
            cap.samples.clone(),
            cap.sample_rate.clone(),
            cap.status.clone(),
            cap.stop.clone(),
        );
        let source = source.clone();
        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                if let Err(e) = run_capture(&source, &samples, &rate, &status, &stop) {
                    *status.lock().unwrap() = format!("Audio error: {e}");
                }
            })
            .ok()?;
    }
    #[cfg(not(feature = "audio"))]
    {
        *cap.status.lock().unwrap() = "Built without the `audio` feature".into();
    }
    Some(cap)
}

#[cfg(feature = "audio")]
fn run_capture(
    source: &AudioSource,
    samples: &Arc<Mutex<Vec<f32>>>,
    rate: &Arc<Mutex<f32>>,
    status: &Arc<Mutex<String>>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{SampleFormat, SizedSample};
    let host = cpal::default_host();
    let device = match source {
        AudioSource::System => {
            if cfg!(windows) {
                host.default_output_device()
            } else {
                host.default_input_device()
            }
        }
        AudioSource::Input(name) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.description().map(|x| x.name() == name).unwrap_or(false)),
        _ => None,
    }
    .ok_or("device not found")?;
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "audio".into());
    let config = if device.supports_input() {
        device.default_input_config()
    } else {
        device.default_output_config()
    }
    .map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    *rate.lock().unwrap() = config.sample_rate() as f32;

    fn push<T: SizedSample + Copy>(
        data: &[T],
        channels: usize,
        buf: &Arc<Mutex<Vec<f32>>>,
        conv: fn(T) -> f32,
    ) {
        if let Ok(mut b) = buf.lock() {
            for frame in data.chunks(channels.max(1)) {
                let s: f32 = frame.iter().map(|&x| conv(x)).sum::<f32>() / frame.len() as f32;
                b.push(s);
            }
            let keep = FFT_SIZE * 2;
            if b.len() > keep {
                let drop = b.len() - keep;
                b.drain(..drop);
            }
        }
    }

    let err = |e| eprintln!("audio stream error: {e}");
    let b = samples.clone();
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            config.into(),
            move |d: &[f32], _: &_| push(d, channels, &b, |x| x),
            err,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config.into(),
            move |d: &[i16], _: &_| push(d, channels, &b, |x| x as f32 / 32768.0),
            err,
            None,
        ),
        SampleFormat::I32 => device.build_input_stream(
            config.into(),
            move |d: &[i32], _: &_| push(d, channels, &b, |x| x as f32 / 2_147_483_648.0),
            err,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            config.into(),
            move |d: &[u16], _: &_| push(d, channels, &b, |x| (x as f32 - 32768.0) / 32768.0),
            err,
            None,
        ),
        other => return Err(format!("unsupported sample format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    *status.lock().unwrap() = format!("Listening: {name}");
    // Keep the stream alive on this thread until asked to stop.
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stream);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_kicks_in_a_synthetic_signal() {
        let sr = 48000.0;
        let mut a = Analyzer::default();
        let mut history = Vec::new();
        let dt = 1.0 / 60.0;
        let mut t = 0.0f32;
        for _ in 0..(60 * 4) {
            // Advance 1/60 s of audio: 120 BPM kick (60 Hz thump) plus hiss.
            let n = (sr * dt) as usize;
            for _ in 0..n {
                let since = (t * 2.0).fract() / 2.0;
                let kick = (-since * 25.0).exp() * (std::f32::consts::TAU * 60.0 * t).sin();
                let hiss = ((t * 12345.678).sin() * 43758.545).fract() * 0.02;
                history.push(kick * 0.8 + hiss);
                t += 1.0 / sr;
            }
            a.process(&history, sr, dt);
        }
        let beats = a.features.beat_count;
        assert!(
            (5..=10).contains(&beats),
            "expected ~8 beats in 4 s at 120 BPM, got {beats}"
        );
    }
}
