//! Persistent app configuration.

use crate::audio::AudioSource;
use crate::devices::DeviceConfig;
use crate::effects::Program;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Pixels in the virtual canvas every device samples from.
    pub canvas_len: usize,
    pub master: f32,
    pub speed: f32,
    pub bpm: f32,
    pub audio: AudioSource,
    /// Let detected beats set the tempo and phase.
    pub follow_audio_tempo: bool,
    /// LAN address to send multicast from, for PCs with VPN/Hyper-V/WSL adapters.
    pub iface: String,
    pub power_on_when_live: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            canvas_len: 120,
            master: 1.0,
            speed: 1.0,
            bpm: 120.0,
            audio: AudioSource::Off,
            follow_audio_tempo: true,
            iface: String::new(),
            power_on_when_live: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub settings: Settings,
    pub devices: Vec<DeviceConfig>,
    pub program: Program,
    /// Bumped by the UI to restart the beat clock on the downbeat (tap tempo).
    #[serde(skip)]
    pub beat_resync: u64,
}

pub fn path() -> Option<PathBuf> {
    let d = directories::ProjectDirs::from("", "", "ReadySetGlow")?
        .config_dir()
        .to_path_buf();
    std::fs::create_dir_all(&d).ok()?;
    Some(d.join("config.json"))
}

pub fn load() -> Option<Config> {
    serde_json::from_slice(&std::fs::read(path()?).ok()?).ok()
}

pub fn save(c: &Config) {
    if let Some(p) = path() {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(c).expect("json")).is_ok() {
            let _ = std::fs::rename(tmp, p);
        }
    }
}
