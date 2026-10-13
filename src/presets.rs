//! Built-in programs and user preset storage.

use crate::color::{BlendMode, Rgb};
use crate::effects::{EffectKind as K, Layer, ModShape, ModSource, ModTarget, Modulator, Program};
use crate::palette::{Palette, builtin};
use std::path::{Path, PathBuf};

fn pal(name: &str) -> Palette {
    builtin(name).expect("builtin palette")
}

fn layer(kind: K, f: impl FnOnce(&mut Layer)) -> Layer {
    let mut l = Layer::new(kind);
    f(&mut l);
    l
}

fn prog(name: &str, layers: Vec<Layer>) -> Program {
    Program {
        name: name.into(),
        layers,
    }
}

pub fn builtins() -> Vec<Program> {
    vec![
        prog(
            "Neon Drift",
            vec![
                layer(K::NoiseFlow, |l| {
                    l.params.palette = pal("Synthwave");
                    l.params.speed = 0.08;
                    l.params.scale = 1.2;
                }),
                layer(K::Sparkle, |l| {
                    l.params.palette = pal("Ice");
                    l.params.density = 0.25;
                    l.opacity = 0.7;
                }),
            ],
        ),
        prog(
            "Campfire",
            vec![
                layer(K::Fire, |l| {
                    l.params.mirror = true;
                    l.params.speed = 0.8;
                }),
                layer(K::Sparkle, |l| {
                    l.params.palette = pal("Embers");
                    l.params.density = 0.15;
                    l.params.decay = 0.7;
                    l.opacity = 0.6;
                }),
            ],
        ),
        prog(
            "Northern Lights",
            vec![
                layer(K::Aurora, |l| {
                    l.params.speed = 0.06;
                    l.params.scale = 1.5;
                }),
                layer(K::Sparkle, |l| {
                    l.params.palette = pal("Ice");
                    l.params.density = 0.1;
                    l.params.decay = 0.3;
                    l.opacity = 0.5;
                }),
            ],
        ),
        prog(
            "Thunderstorm",
            vec![
                layer(K::NoiseFlow, |l| {
                    l.params.palette = Palette::from_hex(&[0x020414, 0x0a1440, 0x101a5a], true);
                    l.params.speed = 0.05;
                }),
                layer(K::Ripple, |l| {
                    l.params.palette = Palette::from_hex(&[0x99aaff, 0xffffff], true);
                    l.params.density = 0.15;
                    l.params.decay = 0.8;
                }),
                layer(K::Strobe, |l| {
                    l.params.color = Rgb::from_hex(0xdde4ff);
                    l.params.speed = 6.0;
                    l.params.size = 0.3;
                    l.blend = BlendMode::Add;
                    l.modulator = Modulator {
                        source: ModSource::Wander,
                        target: ModTarget::Opacity,
                        rate: 0.6,
                        depth: 1.0,
                        ..Default::default()
                    };
                    l.opacity = 0.9;
                }),
            ],
        ),
        prog(
            "Lava Lounge",
            vec![layer(K::LavaLamp, |l| {
                l.params.count = 5;
                l.params.size = 0.4;
            })],
        ),
        prog(
            "Ocean Breeze",
            vec![
                layer(K::Gradient, |l| {
                    l.params.palette = pal("Ocean");
                    l.params.speed = 0.04;
                    l.params.scale = 0.8;
                }),
                layer(K::Ripple, |l| {
                    l.params.palette = pal("Ice");
                    l.params.density = 0.2;
                    l.opacity = 0.6;
                }),
            ],
        ),
        prog(
            "Cyber Scanner",
            vec![
                layer(K::Gradient, |l| {
                    l.params.palette = pal("Cyberpunk");
                    l.params.speed = 0.03;
                    l.opacity = 0.25;
                }),
                layer(K::Comet, |l| {
                    l.params.palette = Palette::from_hex(&[0xff0030], true);
                    l.params.count = 1;
                    l.params.bounce = true;
                    l.params.size = 0.35;
                    l.params.speed = 0.5;
                }),
            ],
        ),
        prog(
            "Disco Theatre",
            vec![
                layer(K::Rainbow, |l| {
                    l.opacity = 0.18;
                    l.params.speed = 0.1;
                }),
                layer(K::Theater, |l| {
                    l.params.palette = pal("Candy");
                    l.params.count = 3;
                    l.params.speed = 0.6;
                }),
            ],
        ),
        prog(
            "Rave",
            vec![
                layer(K::BeatPulse, |l| {
                    l.params.palette = pal("Rainbow");
                    l.params.decay = 0.6;
                }),
                layer(K::Spectrum, |l| {
                    l.params.mirror = true;
                    l.params.palette = pal("Synthwave");
                    l.blend = BlendMode::Add;
                    l.opacity = 0.8;
                }),
                layer(K::Strobe, |l| {
                    l.tempo_sync = true;
                    l.params.speed = 1.0;
                    l.params.size = 0.08;
                    l.opacity = 0.35;
                    l.blend = BlendMode::Add;
                }),
            ],
        ),
        prog(
            "Bass Cannon",
            vec![
                layer(K::BassGlow, |l| {
                    l.params.palette = pal("Lava");
                    l.opacity = 0.7;
                }),
                layer(K::BeatComets, |l| {
                    l.params.palette = pal("Cyberpunk");
                    l.params.mirror = true;
                }),
            ],
        ),
        prog(
            "Heartbeat",
            vec![layer(K::Heartbeat, |l| {
                l.tempo_sync = true;
                l.params.speed = 1.0;
            })],
        ),
        prog("Emergency", vec![layer(K::Police, |_| {})]),
        prog(
            "Chill Breathe",
            vec![layer(K::Breathe, |l| {
                l.params.palette = pal("Sunset");
                l.params.speed = 0.08;
                l.params.size = 0.25;
            })],
        ),
        prog(
            "Toxic Plasma",
            vec![layer(K::Plasma, |l| {
                l.params.palette = pal("Toxic");
                l.params.speed = 0.15;
                l.params.scale = 1.5;
            })],
        ),
        prog(
            "Party Wipe",
            vec![layer(K::Wipe, |l| {
                l.params.palette = pal("Pride");
                l.params.count = 6;
                l.tempo_sync = true;
                l.params.speed = 0.5;
            })],
        ),
        prog(
            "Pride Parade",
            vec![layer(K::Gradient, |l| {
                l.params.palette = pal("Pride");
                l.params.speed = 0.08;
            })],
        ),
        prog(
            "Cosy Candle",
            vec![layer(K::Candle, |l| {
                l.params.intensity = 0.6;
            })],
        ),
        prog(
            "Hue Wobble",
            vec![layer(K::Solid, |l| {
                l.params.color = Rgb::from_hex(0xff0066);
                l.modulator = Modulator {
                    source: ModSource::Lfo,
                    shape: ModShape::Triangle,
                    target: ModTarget::Hue,
                    rate: 0.05,
                    depth: 1.0,
                    ..Default::default()
                };
            })],
        ),
    ]
}

pub fn user_dir() -> Option<PathBuf> {
    let d = directories::ProjectDirs::from("", "", "ReadySetGlow")?
        .config_dir()
        .join("presets");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

fn safe_file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() { "preset".into() } else { s }
}

pub fn load_user(dir: &Path) -> Vec<Program> {
    let mut v: Vec<Program> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    v.sort_by_key(|a| a.name.to_lowercase());
    v
}

pub fn save_user(dir: &Path, p: &Program) -> std::io::Result<()> {
    let path = dir.join(format!("{}.json", safe_file_name(&p.name)));
    std::fs::write(path, serde_json::to_vec_pretty(p).expect("json"))
}

pub fn delete_user(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::remove_file(dir.join(format!("{}.json", safe_file_name(name))))
}

/// Compact share string: `rsglow:` + base64(JSON). Fits in a chat message.
pub fn to_share_code(p: &Program) -> String {
    use base64::Engine;
    let json = serde_json::to_vec(p).expect("json");
    format!(
        "rsglow:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    )
}

pub fn from_share_code(s: &str) -> Result<Program, String> {
    use base64::Engine;
    let s = s.trim();
    if s.starts_with('{') {
        return serde_json::from_str(s).map_err(|e| e.to_string());
    }
    let body = s
        .strip_prefix("rsglow:")
        .ok_or("not a readysetglow preset code")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|e| e.to_string())?;
    let mut p: Program = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    // Fresh ids so a pasted preset never collides with layers already loaded.
    for l in &mut p.layers {
        l.id = crate::effects::next_id();
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_code_roundtrip() {
        for p in builtins() {
            let code = to_share_code(&p);
            let back = from_share_code(&code).unwrap();
            assert_eq!(back.name, p.name);
            assert_eq!(back.layers.len(), p.layers.len());
        }
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(safe_file_name("../../etc/passwd"), "______etc_passwd");
        assert_eq!(safe_file_name("   "), "preset");
    }
}
