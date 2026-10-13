//! Colour maths: a linear-ish float RGB type, HSV, blend modes and Kelvin.

use serde::{Deserialize, Serialize};
use std::ops::{Add, Mul};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0.0, 0.0, 0.0);
    pub const WHITE: Rgb = Rgb::new(1.0, 1.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    pub fn from_u8(r: u8, g: u8, b: u8) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }

    pub fn from_hex(hex: u32) -> Self {
        Self::from_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    pub fn to_u8(self) -> [u8; 3] {
        let c = self.clamp();
        [
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
        ]
    }

    pub fn clamp(self) -> Self {
        Self::new(
            self.r.clamp(0.0, 1.0),
            self.g.clamp(0.0, 1.0),
            self.b.clamp(0.0, 1.0),
        )
    }

    /// `h` in turns (0..1), `s` and `v` in 0..1.
    pub fn hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(1.0) * 6.0;
        let i = h.floor();
        let f = h - i;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        match i as i32 {
            0 => Self::new(v, t, p),
            1 => Self::new(q, v, p),
            2 => Self::new(p, v, t),
            3 => Self::new(p, q, v),
            4 => Self::new(t, p, v),
            _ => Self::new(v, p, q),
        }
    }

    pub fn to_hsv(self) -> (f32, f32, f32) {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let d = max - min;
        let h = if d <= 1e-6 {
            0.0
        } else if max == self.r {
            ((self.g - self.b) / d).rem_euclid(6.0) / 6.0
        } else if max == self.g {
            ((self.b - self.r) / d + 2.0) / 6.0
        } else {
            ((self.r - self.g) / d + 4.0) / 6.0
        };
        let s = if max <= 1e-6 { 0.0 } else { d / max };
        (h, s, max)
    }

    pub fn lerp(self, o: Rgb, t: f32) -> Rgb {
        Rgb::new(
            self.r + (o.r - self.r) * t,
            self.g + (o.g - self.g) * t,
            self.b + (o.b - self.b) * t,
        )
    }

    pub fn max_channel(self) -> f32 {
        self.r.max(self.g).max(self.b)
    }

    /// Rec.709 luma, used when an RGB frame has to drive a white-only bulb.
    pub fn luma(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    /// Squared distance in u8 space; cheap "has this changed enough to resend" test.
    pub fn dist2_u8(a: [u8; 3], b: [u8; 3]) -> i32 {
        let dr = a[0] as i32 - b[0] as i32;
        let dg = a[1] as i32 - b[1] as i32;
        let db = a[2] as i32 - b[2] as i32;
        dr * dr + dg * dg + db * db
    }

    /// Approximate black-body colour (Tanner Helland's fit), 1000-40000 K.
    pub fn from_kelvin(k: f32) -> Rgb {
        let t = k.clamp(1000.0, 40000.0) / 100.0;
        let r = if t <= 66.0 {
            255.0
        } else {
            329.698_73 * (t - 60.0).powf(-0.133_204_76)
        };
        let g = if t <= 66.0 {
            99.470_8 * t.ln() - 161.119_57
        } else {
            288.122_16 * (t - 60.0).powf(-0.075_514_85)
        };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.517_73 * (t - 10.0).ln() - 305.044_8
        };
        Rgb::new(r / 255.0, g / 255.0, b / 255.0).clamp()
    }

    /// Estimate a colour temperature that "feels like" this colour, so warm
    /// effects (fire, candle) still read as warm on white-only bulbs.
    pub fn approx_kelvin(self, min_k: f32, max_k: f32) -> f32 {
        let sum = self.r + self.g + self.b;
        if sum < 1e-4 {
            return (min_k + max_k) * 0.5;
        }
        // Warmth from the red-vs-blue balance.
        let warmth = ((self.r - self.b) / sum * 1.5 + 0.5).clamp(0.0, 1.0);
        max_k + (min_k - max_k) * warmth
    }
}

impl Add for Rgb {
    type Output = Rgb;
    fn add(self, o: Rgb) -> Rgb {
        Rgb::new(self.r + o.r, self.g + o.g, self.b + o.b)
    }
}

impl Mul<f32> for Rgb {
    type Output = Rgb;
    fn mul(self, k: f32) -> Rgb {
        Rgb::new(self.r * k, self.g * k, self.b * k)
    }
}

impl Mul for Rgb {
    type Output = Rgb;
    fn mul(self, o: Rgb) -> Rgb {
        Rgb::new(self.r * o.r, self.g * o.g, self.b * o.b)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Screen,
    Multiply,
    Lighten,
    Darken,
    Subtract,
    /// Uses the layer's brightness to cut the layers below (a light "stencil").
    Mask,
}

impl BlendMode {
    pub const ALL: [BlendMode; 8] = [
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::Multiply,
        BlendMode::Lighten,
        BlendMode::Darken,
        BlendMode::Subtract,
        BlendMode::Mask,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Add => "Add",
            BlendMode::Screen => "Screen",
            BlendMode::Multiply => "Multiply",
            BlendMode::Lighten => "Lighten",
            BlendMode::Darken => "Darken",
            BlendMode::Subtract => "Subtract",
            BlendMode::Mask => "Mask",
        }
    }

    /// Blend `top` onto `base`. `alpha` is the layer opacity times the
    /// per-pixel coverage the effect reported.
    pub fn apply(self, base: Rgb, top: Rgb, alpha: f32) -> Rgb {
        if alpha <= 0.0 {
            return base;
        }
        let mixed = match self {
            BlendMode::Normal => top,
            BlendMode::Add => base + top,
            BlendMode::Screen => Rgb::new(
                1.0 - (1.0 - base.r) * (1.0 - top.r),
                1.0 - (1.0 - base.g) * (1.0 - top.g),
                1.0 - (1.0 - base.b) * (1.0 - top.b),
            ),
            BlendMode::Multiply => base * top,
            BlendMode::Lighten => Rgb::new(base.r.max(top.r), base.g.max(top.g), base.b.max(top.b)),
            BlendMode::Darken => Rgb::new(base.r.min(top.r), base.g.min(top.g), base.b.min(top.b)),
            BlendMode::Subtract => Rgb::new(
                (base.r - top.r).max(0.0),
                (base.g - top.g).max(0.0),
                (base.b - top.b).max(0.0),
            ),
            BlendMode::Mask => base * top.max_channel(),
        };
        base.lerp(mixed, alpha.clamp(0.0, 1.0)).clamp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_roundtrip() {
        for &(r, g, b) in &[(1.0, 0.0, 0.0), (0.2, 0.7, 0.4), (0.1, 0.1, 0.9)] {
            let c = Rgb::new(r, g, b);
            let (h, s, v) = c.to_hsv();
            let back = Rgb::hsv(h, s, v);
            assert!(
                (back.r - r).abs() < 1e-4 && (back.g - g).abs() < 1e-4 && (back.b - b).abs() < 1e-4
            );
        }
    }

    #[test]
    fn kelvin_warm_is_red() {
        let warm = Rgb::from_kelvin(2000.0);
        let cool = Rgb::from_kelvin(9000.0);
        assert!(warm.r > warm.b && cool.b > warm.b);
        assert!(Rgb::new(1.0, 0.4, 0.0).approx_kelvin(2000.0, 9000.0) < 3500.0);
    }
}
