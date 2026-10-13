//! Gradient palettes: a sorted list of colour stops sampled with wrap-around.

use crate::color::Rgb;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub pos: f32,
    pub color: Rgb,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub stops: Vec<Stop>,
    /// When true the gradient loops back from the last stop to the first,
    /// which is what you want for anything that scrolls.
    #[serde(default = "yes")]
    pub wrap: bool,
}

fn yes() -> bool {
    true
}

impl Default for Palette {
    fn default() -> Self {
        builtin("Sunset").unwrap()
    }
}

impl Palette {
    pub fn from_hex(hexes: &[u32], wrap: bool) -> Self {
        let n = hexes.len().max(1);
        let stops = hexes
            .iter()
            .enumerate()
            .map(|(i, &h)| Stop {
                pos: if wrap {
                    i as f32 / n as f32
                } else {
                    i as f32 / (n - 1).max(1) as f32
                },
                color: Rgb::from_hex(h),
            })
            .collect();
        Self { stops, wrap }
    }

    pub fn sort(&mut self) {
        self.stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    }

    /// Sample at `t` (any real; wraps or clamps depending on `wrap`).
    pub fn sample(&self, t: f32) -> Rgb {
        let s = &self.stops;
        match s.len() {
            0 => return Rgb::BLACK,
            1 => return s[0].color,
            _ => {}
        }
        let t = if self.wrap {
            t.rem_euclid(1.0)
        } else {
            t.clamp(0.0, 1.0)
        };
        // Stops are kept sorted by the editor; tolerate unsorted input anyway.
        let first = &s[0];
        let last = &s[s.len() - 1];
        if t < first.pos {
            if !self.wrap {
                return first.color;
            }
            let span = first.pos + 1.0 - last.pos;
            let k = if span > 1e-6 {
                (t + 1.0 - last.pos) / span
            } else {
                0.0
            };
            return last.color.lerp(first.color, k);
        }
        for w in s.windows(2) {
            if t >= w[0].pos && t <= w[1].pos {
                let span = w[1].pos - w[0].pos;
                let k = if span > 1e-6 {
                    (t - w[0].pos) / span
                } else {
                    0.0
                };
                return w[0].color.lerp(w[1].color, k);
            }
        }
        if !self.wrap {
            return last.color;
        }
        let span = first.pos + 1.0 - last.pos;
        let k = if span > 1e-6 {
            (t - last.pos) / span
        } else {
            0.0
        };
        last.color.lerp(first.color, k)
    }
}

pub const BUILTIN_NAMES: &[&str] = &[
    "Rainbow",
    "Sunset",
    "Ocean",
    "Forest",
    "Fire",
    "Lava",
    "Aurora",
    "Synthwave",
    "Ice",
    "Candy",
    "Toxic",
    "Pride",
    "Trans",
    "Embers",
    "Cyberpunk",
    "Police",
];

pub fn builtin(name: &str) -> Option<Palette> {
    let p = match name {
        "Rainbow" => Palette::from_hex(
            &[0xff0000, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff],
            true,
        ),
        "Sunset" => Palette::from_hex(&[0xff5e00, 0xff0055, 0x8a00d4, 0x2a00ff, 0xffb300], true),
        "Ocean" => Palette::from_hex(&[0x001a66, 0x0066ff, 0x00ffd5, 0x0033aa], true),
        "Forest" => Palette::from_hex(&[0x0a3d00, 0x2bff00, 0x007a33, 0xc8ff00], true),
        "Fire" => Palette::from_hex(&[0x000000, 0x8b0000, 0xff4500, 0xffb000, 0xffffcc], false),
        "Lava" => Palette::from_hex(&[0x200000, 0xff2a00, 0xff8800, 0x5a0000], true),
        "Aurora" => Palette::from_hex(&[0x00ff88, 0x00b3ff, 0x7a00ff, 0x00ffcc], true),
        "Synthwave" => Palette::from_hex(&[0xff00aa, 0x7700ff, 0x00e5ff, 0xff6a00], true),
        "Ice" => Palette::from_hex(&[0xffffff, 0x99e6ff, 0x0077ff, 0xccf2ff], true),
        "Candy" => Palette::from_hex(&[0xff66cc, 0x66ffff, 0xffff66, 0xcc66ff], true),
        "Toxic" => Palette::from_hex(&[0x39ff14, 0x003300, 0xccff00, 0x008800], true),
        "Pride" => Palette::from_hex(
            &[0xe40303, 0xff8c00, 0xffed00, 0x008026, 0x004dff, 0x750787],
            true,
        ),
        "Trans" => Palette::from_hex(&[0x5bcefa, 0xf5a9b8, 0xffffff, 0xf5a9b8], true),
        "Embers" => Palette::from_hex(&[0x1a0500, 0xff3300, 0x661100, 0xff9900], true),
        "Cyberpunk" => Palette::from_hex(&[0xfcee0a, 0x00f0ff, 0xff003c, 0x00f0ff], true),
        "Police" => Palette::from_hex(&[0xff0000, 0x0000ff], true),
        _ => return None,
    };
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_smoothly() {
        let p = Palette::from_hex(&[0xff0000, 0x0000ff], true);
        // Halfway between the last stop (0.5) and wrapping to the first (1.0).
        let c = p.sample(0.75);
        assert!((c.r - 0.5).abs() < 0.01 && (c.b - 0.5).abs() < 0.01);
        assert_eq!(p.sample(1.0), p.sample(0.0));
    }

    #[test]
    fn all_builtins_exist() {
        for n in BUILTIN_NAMES {
            assert!(builtin(n).is_some(), "{n}");
        }
    }
}
