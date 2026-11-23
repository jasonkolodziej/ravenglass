//! sRGB colours, WCAG contrast, and an OKLCH-lightness contrast fixer.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }

impl Rgb {
    pub const MAGENTA: Rgb = Rgb { r: 1.0, g: 0.0, b: 1.0, a: 1.0 };

    /// `#RGB`, `#RRGGBB`, `#RRGGBBAA` or `rgba(r,g,b,a)`.
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim();
        if let Some(h) = s.strip_prefix('#') {
            let h: String = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect() } else { h.to_owned() };
            let n = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|v| v as f32 / 255.0);
            return match h.len() { 6 => Some(Rgb { r: n(0)?, g: n(2)?, b: n(4)?, a: 1.0 }), 8 => Some(Rgb { r: n(0)?, g: n(2)?, b: n(4)?, a: n(6)? }), _ => None };
        }
        let inner = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?.strip_suffix(')')?;
        let v: Vec<f32> = inner.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        match v.as_slice() { [r, g, b] => Some(Rgb { r: r / 255.0, g: g / 255.0, b: b / 255.0, a: 1.0 }), [r, g, b, a] => Some(Rgb { r: r / 255.0, g: g / 255.0, b: b / 255.0, a: *a }), _ => None }
    }

    pub fn hex(self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(self.r), c(self.g), c(self.b))
    }

    fn lin(c: f32) -> f32 { if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } }
    fn enc(c: f32) -> f32 { if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 } }

    pub fn luminance(self) -> f32 { 0.2126 * Self::lin(self.r) + 0.7152 * Self::lin(self.g) + 0.0722 * Self::lin(self.b) }

    pub fn to_oklab(self) -> [f32; 3] {
        let (r, g, b) = (Self::lin(self.r), Self::lin(self.g), Self::lin(self.b));
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s, 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s, 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s]
    }

    pub fn from_oklab([l, a, b]: [f32; 3], alpha: f32) -> Rgb {
        let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
        let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
        let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
        let r = 4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_;
        let g = -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_;
        let bb = -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_;
        Rgb { r: Self::enc(r.clamp(0.0, 1.0)), g: Self::enc(g.clamp(0.0, 1.0)), b: Self::enc(bb.clamp(0.0, 1.0)), a: alpha }
    }
}

/// WCAG 2.x contrast ratio (1..=21).
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (a.luminance(), b.luminance());
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Nudge `fg`'s OKLab lightness away from `bg` (keeping hue and chroma) until it reaches `min`.
/// Returns `fg` unchanged if it already passes.
pub fn fix_contrast(fg: Rgb, bg: Rgb, min: f32) -> Rgb {
    if contrast(fg, bg) >= min { return fg; }
    let [l0, a, b] = fg.to_oklab();
    let dir = if bg.luminance() > 0.18 { -1.0 } else { 1.0 };
    let mut l = l0;
    for _ in 0..200 {
        l = (l + dir * 0.005).clamp(0.0, 1.0);
        // Reduce chroma near the ends of the lightness range so the colour stays in gamut.
        let k = if !(0.05..=0.95).contains(&l) { 0.5 } else { 1.0 };
        let c = Rgb::from_oklab([l, a * k, b * k], fg.a);
        if contrast(c, bg) >= min { return c; }
    }
    if dir > 0.0 { Rgb { r: 1.0, g: 1.0, b: 1.0, a: fg.a } } else { Rgb { r: 0.0, g: 0.0, b: 0.0, a: fg.a } }
}
