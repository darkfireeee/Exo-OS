//! Embedded ExoOS UI assets (outside `src/` for strict module hygiene).
//!
//! Includes:
//! - 5 crisp anti-aliased 8-bit `.exofont` atlases (Inter + UbuntuMono, full FR/EN charset)
//! - Concentric crystal hexagonal Start emblem (`40x40` & `64x64` RGBA)
//! - 8 harmonized multi-color app icons (`40x40` straight RGBA)
//! - Silk Harmony smooth-transition wallpaper (`960x540` Bayer-dithered RGB565)

use exo_ui::ExoFont;

static FONT_INTER_13_BYTES: &[u8] = include_bytes!("../../../assets/ui/fonts/inter_13.exofont");
static FONT_INTER_15_BYTES: &[u8] = include_bytes!("../../../assets/ui/fonts/inter_15.exofont");
static FONT_INTER_18_BOLD_BYTES: &[u8] =
    include_bytes!("../../../assets/ui/fonts/inter_18_bold.exofont");
static FONT_INTER_26_BOLD_BYTES: &[u8] =
    include_bytes!("../../../assets/ui/fonts/inter_26_bold.exofont");
static FONT_MONO_15_BYTES: &[u8] = include_bytes!("../../../assets/ui/fonts/mono_15.exofont");

pub static ICON_START_40: &[u8] = include_bytes!("../../../assets/ui/icons/start_logo_40.rgba");
pub static ICON_START_64: &[u8] = include_bytes!("../../../assets/ui/icons/start_logo_64.rgba");
pub static ICON_FILES_40: &[u8] = include_bytes!("../../../assets/ui/icons/files_40.rgba");
pub static ICON_TERMINAL_40: &[u8] = include_bytes!("../../../assets/ui/icons/terminal_40.rgba");
pub static ICON_MONITOR_40: &[u8] = include_bytes!("../../../assets/ui/icons/monitor_40.rgba");
pub static ICON_SETTINGS_40: &[u8] = include_bytes!("../../../assets/ui/icons/settings_40.rgba");
pub static ICON_EDITOR_40: &[u8] = include_bytes!("../../../assets/ui/icons/editor_40.rgba");
pub static ICON_BENCH_40: &[u8] = include_bytes!("../../../assets/ui/icons/bench_40.rgba");
pub static ICON_TRASH_40: &[u8] = include_bytes!("../../../assets/ui/icons/trash_40.rgba");
pub static ICON_GAMES_40: &[u8] = include_bytes!("../../../assets/ui/icons/games_40.rgba");

pub static WALLPAPER_HARMONY_RGB565: &[u8] =
    include_bytes!("../../../assets/wallpapers/wallpaper_harmony_960x540.rgb565");
pub const WALLPAPER_HARMONY_W: u32 = 960;
pub const WALLPAPER_HARMONY_H: u32 = 540;

pub const WALLPAPER_NAMES_FR: [&str; 4] = [
    "Soie Harmonie",
    "Horizon Clair",
    "Aurore Sereine",
    "Ardoise Velours",
];

pub const WALLPAPER_NAMES_EN: [&str; 4] = [
    "Silk Harmony",
    "Light Horizon",
    "Serene Dusk",
    "Velvet Slate",
];

#[derive(Clone, Copy)]
pub struct FontSet {
    pub sm: ExoFont<'static>,
    pub md: ExoFont<'static>,
    pub lg_bold: ExoFont<'static>,
    pub hero: ExoFont<'static>,
    pub mono: ExoFont<'static>,
}

impl FontSet {
    pub fn load() -> Self {
        Self {
            sm: ExoFont::from_bytes(FONT_INTER_13_BYTES).expect("valid inter_13.exofont"),
            md: ExoFont::from_bytes(FONT_INTER_15_BYTES).expect("valid inter_15.exofont"),
            lg_bold: ExoFont::from_bytes(FONT_INTER_18_BOLD_BYTES)
                .expect("valid inter_18_bold.exofont"),
            hero: ExoFont::from_bytes(FONT_INTER_26_BOLD_BYTES)
                .expect("valid inter_26_bold.exofont"),
            mono: ExoFont::from_bytes(FONT_MONO_15_BYTES).expect("valid mono_15.exofont"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconId {
    Files,
    Terminal,
    Monitor,
    Settings,
    Editor,
    Bench,
    Trash,
    Games,
}

impl IconId {
    #[inline]
    pub fn rgba_40(self) -> &'static [u8] {
        match self {
            Self::Files => ICON_FILES_40,
            Self::Terminal => ICON_TERMINAL_40,
            Self::Monitor => ICON_MONITOR_40,
            Self::Settings => ICON_SETTINGS_40,
            Self::Editor => ICON_EDITOR_40,
            Self::Bench => ICON_BENCH_40,
            Self::Trash => ICON_TRASH_40,
            Self::Games => ICON_GAMES_40,
        }
    }
}
