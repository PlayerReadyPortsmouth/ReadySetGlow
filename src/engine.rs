//! The engine thread: renders the program at 60 fps, adapts it to every
//! device and sends packets while "Live" is on. The UI only edits `Shared`.

use crate::audio::{self, Analyzer, AudioFeatures, AudioSource};
use crate::color::Rgb;
use crate::config::Config;
use crate::devices::{self, DeviceConfig, DeviceRuntime, OutputMode};
use crate::effects::{Frame, Renderer};
use crate::govee::{self, Net};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct DevicePreview {
    /// What the device is showing (or would show, when not live).
    pub shown: Vec<[u8; 3]>,
    pub msgs_per_sec: f32,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct Preview {
    pub canvas: Vec<Rgb>,
    pub devices: HashMap<String, DevicePreview>,
    pub audio: AudioFeatures,
    pub audio_status: String,
    pub beats: f64,
    pub bpm: f32,
    pub fps: f32,
}

pub struct Shared {
    pub config: Config,
    pub version: u64,
    pub live: bool,
    pub preview: Preview,
    pub quit: bool,
}

struct DevState {
    rt: DeviceRuntime,
    last_count: u64,
    rate: f32,
    error: Option<String>,
}

pub fn spawn(shared: Arc<Mutex<Shared>>, net: Option<Net>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || run(shared, net))
        .expect("spawn engine")
}

fn run(shared: Arc<Mutex<Shared>>, net: Option<Net>) {
    let mut cfg = Config::default();
    let mut version = u64::MAX;
    let mut renderer = Renderer::default();
    let mut canvas: Vec<Rgb> = Vec::new();
    let mut states: HashMap<String, DevState> = HashMap::new();
    let mut was_live = false;

    let mut analyzer = Analyzer::default();
    let mut capture: Option<audio::Capture> = None;
    let mut capture_src = AudioSource::Off;
    let mut samples = Vec::new();
    let mut last_audio_beat = 0u64;
    let mut beat_times: Vec<f64> = Vec::new();

    let mut beats = 0.0f64;
    let mut bpm = 120.0f32;
    let mut resync_seen = 0u64;

    let start = Instant::now();
    let mut last = start;
    let mut stat_t = 0.0f64;
    let mut frames = 0u32;
    let mut fps = 0.0f32;
    let tick = Duration::from_micros(16_667);

    loop {
        let frame_start = Instant::now();
        let now = start.elapsed().as_secs_f64();
        let dt = (frame_start - last).as_secs_f32().clamp(0.0, 0.1);
        last = frame_start;

        let live = {
            let s = shared.lock().unwrap();
            if s.quit {
                drop(s);
                release_all(&cfg, &mut states, net.as_ref(), was_live);
                return;
            }
            if s.version != version {
                version = s.version;
                cfg = s.config.clone();
            }
            s.live
        };

        // Tap tempo: user-set BPM wins unless audio is driving the tempo.
        if cfg.beat_resync != resync_seen {
            resync_seen = cfg.beat_resync;
            beats = beats.floor();
            bpm = cfg.settings.bpm;
        }
        if !(cfg.settings.follow_audio_tempo && audio_on(&cfg.settings.audio)) {
            bpm = cfg.settings.bpm;
        }

        // Audio.
        if cfg.settings.audio != capture_src {
            capture_src = cfg.settings.audio.clone();
            capture = audio::start(&capture_src);
            analyzer = Analyzer::default();
            last_audio_beat = 0;
            beat_times.clear();
        }
        let audio_status = match &capture_src {
            AudioSource::Off => String::new(),
            AudioSource::Demo => {
                analyzer.features = AudioFeatures::demo(now as f32);
                "Demo signal".into()
            }
            _ => match &capture {
                Some(c) => {
                    let sr = c.latest(&mut samples);
                    analyzer.process(&samples, sr, dt);
                    c.status.lock().unwrap().clone()
                }
                None => "Audio unavailable".into(),
            },
        };
        if analyzer.features.beat_count != last_audio_beat {
            last_audio_beat = analyzer.features.beat_count;
            if cfg.settings.follow_audio_tempo && audio_on(&capture_src) {
                beat_times.push(now);
                if beat_times.len() > 12 {
                    beat_times.remove(0);
                }
                if let Some(est) = estimate_bpm(&beat_times) {
                    bpm += (est - bpm) * 0.3;
                }
                // Pull the beat clock onto the detected beat.
                let r = beats.round();
                beats += (r - beats) * 0.5;
            }
        }
        beats += (dt * bpm / 60.0) as f64;

        // Render.
        let n = cfg.settings.canvas_len.clamp(8, 600);
        canvas.resize(n, Rgb::BLACK);
        let f = Frame {
            dt,
            speed: cfg.settings.speed,
            bpm,
            beats,
            audio: &analyzer.features,
        };
        renderer.render(&cfg.program, &f, &mut canvas);

        // Going live / stopping.
        if live != was_live {
            if live {
                for d in cfg.devices.iter().filter(|d| d.enabled) {
                    if let Some(st) = states.get_mut(&d.id) {
                        st.rt = DeviceRuntime::default();
                    }
                    if cfg.settings.power_on_when_live {
                        let _ = send(net.as_ref(), d, &govee::cmd_turn(true));
                    }
                }
            } else {
                release_all(&cfg, &mut states, net.as_ref(), true);
            }
            was_live = live;
        }

        // Devices.
        states.retain(|id, _| cfg.devices.iter().any(|d| &d.id == id));
        let mut previews = HashMap::new();
        for d in &cfg.devices {
            let st = states.entry(d.id.clone()).or_insert_with(|| DevState {
                rt: DeviceRuntime::default(),
                last_count: 0,
                rate: 0.0,
                error: None,
            });
            let px = target_pixels(&canvas, d);
            let msgs = st.rt.plan(d, &px, cfg.settings.master, now, dt);
            if live && d.enabled {
                for m in &msgs {
                    if let Err(e) = send(net.as_ref(), d, m) {
                        st.error = Some(e);
                    } else {
                        st.error = None;
                    }
                }
            }
            previews.insert(
                d.id.clone(),
                DevicePreview {
                    shown: st.rt.shown.clone(),
                    msgs_per_sec: st.rate,
                    error: st.error.clone(),
                },
            );
        }

        frames += 1;
        if now - stat_t >= 1.0 {
            let span = (now - stat_t) as f32;
            fps = frames as f32 / span;
            frames = 0;
            for st in states.values_mut() {
                let sent = st.rt.msgs_sent;
                st.rate = if live {
                    (sent - st.last_count) as f32 / span
                } else {
                    0.0
                };
                st.last_count = sent;
            }
            stat_t = now;
        }

        {
            let mut s = shared.lock().unwrap();
            let p = &mut s.preview;
            p.canvas.clone_from(&canvas);
            p.devices = previews;
            p.audio = analyzer.features.clone();
            p.audio_status = audio_status;
            p.beats = beats;
            p.bpm = bpm;
            p.fps = fps;
        }

        let spent = frame_start.elapsed();
        if spent < tick {
            std::thread::sleep(tick - spent);
        }
    }
}

fn audio_on(src: &AudioSource) -> bool {
    !matches!(src, AudioSource::Off)
}

pub fn target_pixels(canvas: &[Rgb], d: &DeviceConfig) -> Vec<Rgb> {
    match d.mode {
        OutputMode::Segments { .. } | OutputMode::Stream { .. } => devices::sample_span(
            canvas,
            d.span_start,
            d.span_end,
            d.mode.pixel_count(),
            d.reverse,
        ),
        _ => {
            // Sample at a modest resolution, then collapse to one colour.
            let px = devices::sample_span(canvas, d.span_start, d.span_end, 24, d.reverse);
            vec![devices::collapse(&px, d.sample)]
        }
    }
}

fn send(net: Option<&Net>, d: &DeviceConfig, body: &serde_json::Value) -> Result<(), String> {
    let net = net.ok_or("network unavailable")?;
    let ip: IpAddr = d.ip.parse().map_err(|_| format!("bad IP '{}'", d.ip))?;
    net.send(ip, body).map_err(|e| e.to_string())
}

fn release_all(
    cfg: &Config,
    states: &mut HashMap<String, DevState>,
    net: Option<&Net>,
    live: bool,
) {
    for d in &cfg.devices {
        if let Some(st) = states.get_mut(&d.id) {
            for m in st.rt.release(d) {
                if live && d.enabled {
                    let _ = send(net, d, &m);
                }
            }
        }
    }
}

/// Median inter-beat interval, if the recent beats are steady enough.
fn estimate_bpm(times: &[f64]) -> Option<f32> {
    if times.len() < 5 {
        return None;
    }
    let mut iv: Vec<f64> = times
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&d| d > 0.25 && d < 1.5)
        .collect();
    if iv.len() < 4 {
        return None;
    }
    iv.sort_by(|a, b| a.total_cmp(b));
    let med = iv[iv.len() / 2];
    let close = iv.iter().filter(|&&d| (d - med).abs() / med < 0.12).count();
    if close * 3 < iv.len() * 2 {
        return None;
    }
    let mut bpm = 60.0 / med;
    while bpm < 80.0 {
        bpm *= 2.0;
    }
    while bpm > 180.0 {
        bpm /= 2.0;
    }
    Some(bpm as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_estimation() {
        let t: Vec<f64> = (0..8).map(|i| i as f64 * 0.5).collect();
        assert!((estimate_bpm(&t).unwrap() - 120.0).abs() < 0.5);
        // Half-time detection folds into range.
        let t: Vec<f64> = (0..8).map(|i| i as f64 * 1.2).collect();
        assert!((estimate_bpm(&t).unwrap() - 100.0).abs() < 0.5);
        let jitter = [0.0, 0.3, 1.1, 1.4, 2.6, 2.9, 4.0];
        assert!(estimate_bpm(&jitter).is_none());
    }
}
