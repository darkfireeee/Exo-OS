//! ARGB8888 colors, neutral Dark & Light palettes, harmonized accents, and smooth transitions.

/// 32-bit `0xAARRGGBB` color value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const TRANSPARENT: Self = Self(0x0000_0000);
    pub const BLACK: Self = Self(0xFF00_0000);
    pub const WHITE: Self = Self(0xFFFF_FFFF);

    #[inline(always)]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    #[inline(always)]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    #[inline(always)]
    pub const fn with_alpha(self, a: u8) -> Self {
        Self((self.0 & 0x00FF_FFFF) | ((a as u32) << 24))
    }

    #[inline(always)]
    pub const fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }

    #[inline(always)]
    pub const fn r(self) -> u8 {
        ((self.0 >> 16) & 0xFF) as u8
    }

    #[inline(always)]
    pub const fn g(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    #[inline(always)]
    pub const fn b(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// Linearly interpolate between `self` (`t = 0`) and `other` (`t = 255`).
    #[inline(always)]
    pub const fn lerp(self, other: Self, t: u8) -> Self {
        let inv = 255u32 - t as u32;
        let tf = t as u32;
        let a = (((self.a() as u32 * inv + other.a() as u32 * tf + 128) * 257) >> 16) & 0xFF;
        let r = (((self.r() as u32 * inv + other.r() as u32 * tf + 128) * 257) >> 16) & 0xFF;
        let g = (((self.g() as u32 * inv + other.g() as u32 * tf + 128) * 257) >> 16) & 0xFF;
        let b = (((self.b() as u32 * inv + other.b() as u32 * tf + 128) * 257) >> 16) & 0xFF;
        Self((a << 24) | (r << 16) | (g << 8) | b)
    }

    /// Scale RGB channels by fixed-point brightness percentage (`100` = 1.0x).
    #[inline(always)]
    pub const fn scale_brightness(self, pct: u16) -> Self {
        if pct == 100 {
            return self;
        }
        let p = pct as u32;
        let r = (self.r() as u32 * p) / 100;
        let g = (self.g() as u32 * p) / 100;
        let b = (self.b() as u32 * p) / 100;
        let r = if r > 255 { 255 } else { r };
        let g = if g > 255 { 255 } else { g };
        let b = if b > 255 { 255 } else { b };
        Self(((self.a() as u32) << 24) | (r << 16) | (g << 8) | b)
    }
}

/// Smooth scalar linear interpolation for UI transitions.
#[inline(always)]
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Division-free SWAR alpha-blend `src` over `dst` (`0xAARRGGBB` -> `0xFFRRGGBB`).
#[inline(always)]
pub const fn blend_over(dst: u32, src: Color) -> u32 {
    let a = src.a() as u32;
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return 0xFF00_0000 | (src.0 & 0x00FF_FFFF);
    }
    let inv = 255 - a;
    let s_rb = src.0 & 0x00FF_00FF;
    let d_rb = dst & 0x00FF_00FF;
    let rb = s_rb * a + d_rb * inv + 0x0080_0080;
    let rb = ((rb + ((rb >> 8) & 0x00FF_00FF)) >> 8) & 0x00FF_00FF;
    let s_g = (src.0 >> 8) & 0xFF;
    let d_g = (dst >> 8) & 0xFF;
    let g = s_g * a + d_g * inv + 0x80;
    let g = ((g + (g >> 8)) >> 8) & 0xFF;
    0xFF00_0000 | rb | (g << 8)
}

/// Division-free SWAR alpha blend with pre-unpacked `s_rb = src & 0x00FF_00FF`, `s_g = (src >> 8) & 0xFF`.
#[inline(always)]
pub const fn blend_over_pre(dst: u32, s_rb: u32, s_g: u32, a: u32, inv: u32) -> u32 {
    let d_rb = dst & 0x00FF_00FF;
    let rb = s_rb * a + d_rb * inv + 0x0080_0080;
    let rb = ((rb + ((rb >> 8) & 0x00FF_00FF)) >> 8) & 0x00FF_00FF;
    let d_g = (dst >> 8) & 0xFF;
    let g = s_g * a + d_g * inv + 0x80;
    let g = ((g + (g >> 8)) >> 8) & 0xFF;
    0xFF00_0000 | rb | (g << 8)
}

/// Fast division-free alpha blend with extra coverage `cov` (`0..=255`) for anti-aliased edges/glyphs.
#[inline(always)]
pub const fn blend_over_cov(dst: u32, src: Color, cov: u8) -> u32 {
    if cov == 0 {
        return dst;
    }
    let sa = src.a() as u32;
    let eff_a = if sa == 255 {
        cov as u32
    } else {
        ((sa * cov as u32 + 128) * 257) >> 16
    };
    if eff_a == 0 {
        return dst;
    }
    if eff_a == 255 {
        return 0xFF00_0000 | (src.0 & 0x00FF_FFFF);
    }
    blend_over_pre(dst, src.0 & 0x00FF_00FF, (src.0 >> 8) & 0xFF, eff_a, 255 - eff_a)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationMode {
    Full,
    Reduced,
    Off,
}

/// Six harmonized accent colors (no monochrome blue lock-in).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccentPreset {
    Indigo,
    Emerald,
    Coral,
    Amber,
    Sky,
    Plum,
}

impl AccentPreset {
    pub const ALL: [Self; 6] = [
        Self::Indigo,
        Self::Emerald,
        Self::Coral,
        Self::Amber,
        Self::Sky,
        Self::Plum,
    ];

    pub const fn name(self) -> &'static str {
        self.name_i18n(true)
    }

    pub const fn name_i18n(self, is_en: bool) -> &'static str {
        match (self, is_en) {
            (Self::Indigo, _) => "Indigo",
            (Self::Emerald, true) => "Emerald",
            (Self::Emerald, false) => "Émeraude",
            (Self::Coral, true) => "Coral",
            (Self::Coral, false) => "Corail",
            (Self::Amber, true) => "Amber",
            (Self::Amber, false) => "Ambre",
            (Self::Sky, true) => "Azure",
            (Self::Sky, false) => "Azur",
            (Self::Plum, true) => "Plum",
            (Self::Plum, false) => "Prune",
        }
    }

    pub const fn primary(self) -> Color {
        match self {
            Self::Indigo => Color::rgb(0x63, 0x66, 0xF1),
            Self::Emerald => Color::rgb(0x10, 0xB9, 0x81),
            Self::Coral => Color::rgb(0xF4, 0x3F, 0x5E),
            Self::Amber => Color::rgb(0xF5, 0x9E, 0x0B),
            Self::Sky => Color::rgb(0x0E, 0xA5, 0xE9),
            Self::Plum => Color::rgb(0xA8, 0x55, 0xF7),
        }
    }
}

/// Complete neutral surface & semantic color palette resolved for Dark or Light mode.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub is_dark: bool,
    pub accent: Color,
    pub window_bg: Color,
    pub sidebar_bg: Color,
    pub card_bg: Color,
    pub elevated_bg: Color,
    pub input_bg: Color,
    pub hover_bg: Color,
    pub dock_bg: Color,
    pub topbar_bg: Color,
    pub menu_bg: Color,
    pub border_subtle: Color,
    pub border_strong: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    pub terminal_bg: Color,
}

/// Global display, theme, and transition configuration.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub mode: ThemeMode,
    pub accent: AccentPreset,
    /// Global display brightness percentage (`35..=130`, default `100`).
    pub brightness_pct: u16,
    /// Window translucency (`200..=255`, default `242` for crisp neutral surfaces).
    pub surface_alpha: u8,
    /// Wallpaper index (`0` = Soie Harmonie, `1` = Horizon Clair, `2` = Aurore Sereine, `3` = Ardoise Velours).
    pub wallpaper_idx: u8,
    /// Optional backdrop blur under dock/menus.
    pub blur_enabled: bool,
    /// Animation smoothness preset.
    pub animations: AnimationMode,
    /// High-contrast accessibility borders.
    pub high_contrast: bool,
    /// Warm evening blue-light filter (Night Light).
    pub night_light: bool,
    /// Developer FPS/render overlay.
    pub show_fps: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new()
    }
}

impl Theme {
    pub const fn new() -> Self {
        Self {
            mode: ThemeMode::Dark,
            accent: AccentPreset::Indigo,
            brightness_pct: 100,
            surface_alpha: 244,
            wallpaper_idx: 0,
            blur_enabled: true,
            animations: AnimationMode::Full,
            high_contrast: false,
            night_light: false,
            show_fps: false,
        }
    }

    #[inline]
    pub const fn accent_primary(&self) -> Color {
        self.accent.primary()
    }

    /// Resolve a clean, neutral Dark or Light palette (never heavy blue-tinted glass).
    /// When `surface_alpha >= 235` (opaque cached-window mode), interior window surfaces use
    /// `alpha = 255` so every card/sidebar/editor fill hits fast `slice::fill` (15x faster).
    pub const fn palette(&self) -> Palette {
        let opaque = self.surface_alpha >= 235;
        let a = if opaque { 255 } else { self.surface_alpha };
        let ca = if opaque { 255 } else { 0xF2 };
        let ea = if opaque { 255 } else { 0xF5 };
        let ta = if opaque { 255 } else { 0xF8 };
        let acc = self.accent.primary();
        match self.mode {
            ThemeMode::Dark => Palette {
                is_dark: true,
                accent: acc,
                window_bg: Color::rgba(0x16, 0x18, 0x1D, a),
                sidebar_bg: Color::rgba(0x12, 0x14, 0x19, a),
                card_bg: Color::rgba(0x1D, 0x20, 0x28, ca),
                elevated_bg: Color::rgba(0x25, 0x29, 0x33, ea),
                input_bg: Color::rgba(0x0F, 0x11, 0x15, ea),
                hover_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0x14),
                dock_bg: Color::rgba(0x18, 0x1A, 0x20, 0xDF),
                topbar_bg: Color::rgba(0x12, 0x14, 0x19, 0xD4),
                menu_bg: Color::rgba(0x1B, 0x1E, 0x25, 0xF4),
                border_subtle: if self.high_contrast {
                    Color::rgba(0xFF, 0xFF, 0xFF, 0x55)
                } else {
                    Color::rgba(0xFF, 0xFF, 0xFF, 0x1A)
                },
                border_strong: if self.high_contrast {
                    Color::rgba(0xFF, 0xFF, 0xFF, 0x99)
                } else {
                    Color::rgba(0xFF, 0xFF, 0xFF, 0x30)
                },
                text_primary: Color::rgb(0xF3, 0xF4, 0xF6),
                text_secondary: Color::rgb(0xA1, 0xA8, 0xB8),
                text_muted: Color::rgb(0x6E, 0x76, 0x87),
                success: Color::rgb(0x10, 0xB9, 0x81),
                warning: Color::rgb(0xF5, 0x9E, 0x0B),
                danger: Color::rgb(0xF4, 0x3F, 0x5E),
                info: Color::rgb(0x38, 0xBD, 0xF8),
                terminal_bg: Color::rgba(0x10, 0x12, 0x16, ta),
            },
            ThemeMode::Light => Palette {
                is_dark: false,
                accent: acc,
                window_bg: Color::rgba(0xFA, 0xFA, 0xFC, a),
                sidebar_bg: Color::rgba(0xF1, 0xF3, 0xF7, a),
                card_bg: Color::rgba(0xFF, 0xFF, 0xFF, if opaque { 255 } else { 0xFA }),
                elevated_bg: Color::rgba(0xF3, 0xF5, 0xF9, if opaque { 255 } else { 0xFA }),
                input_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0xFF),
                hover_bg: Color::rgba(0x11, 0x18, 0x27, 0x0E),
                dock_bg: Color::rgba(0xF7, 0xF8, 0xFB, 0xDF),
                topbar_bg: Color::rgba(0xF6, 0xF7, 0xFA, 0xDC),
                menu_bg: Color::rgba(0xFF, 0xFF, 0xFF, 0xF8),
                border_subtle: if self.high_contrast {
                    Color::rgba(0x11, 0x18, 0x27, 0x45)
                } else {
                    Color::rgba(0x11, 0x18, 0x27, 0x16)
                },
                border_strong: if self.high_contrast {
                    Color::rgba(0x11, 0x18, 0x27, 0x88)
                } else {
                    Color::rgba(0x11, 0x18, 0x27, 0x2C)
                },
                text_primary: Color::rgb(0x14, 0x17, 0x1F),
                text_secondary: Color::rgb(0x4B, 0x55, 0x63),
                text_muted: Color::rgb(0x7C, 0x86, 0x98),
                success: Color::rgb(0x05, 0x96, 0x69),
                warning: Color::rgb(0xD9, 0x77, 0x06),
                danger: Color::rgb(0xE1, 0x1D, 0x48),
                info: Color::rgb(0x02, 0x84, 0xC7),
                terminal_bg: Color::rgba(0x16, 0x18, 0x1E, if opaque { 255 } else { 0xFA }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_endpoints_and_midpoint() {
        let dst = Color::rgb(0, 0, 0).0;
        assert_eq!(blend_over(dst, Color::rgba(255, 128, 64, 0)), dst);
        assert_eq!(
            blend_over(dst, Color::rgba(255, 128, 64, 255)),
            Color::rgb(255, 128, 64).0
        );
        let mid = Color(blend_over(Color::rgb(0, 0, 0).0, Color::rgba(200, 100, 50, 128)));
        assert!((mid.r() as i32 - 100).abs() <= 1);
        assert!((mid.g() as i32 - 50).abs() <= 1);
    }

    #[test]
    fn dark_and_light_palettes_have_distinct_contrast() {
        let mut t = Theme::new();
        let dark = t.palette();
        t.mode = ThemeMode::Light;
        let light = t.palette();
        assert!(dark.text_primary.r() > 200);
        assert!(light.text_primary.r() < 40);
        assert!(light.window_bg.r() > 230);
    }
}
