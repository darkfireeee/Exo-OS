//! Live bilingual internationalization (Français / English) per ExoOS UI Spec §0 & §1.14.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    #[inline]
    pub const fn is_en(self) -> bool {
        matches!(self, Self::En)
    }

    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Fr => "FR",
            Self::En => "EN",
        }
    }

    /// Pick French or English string inline with zero allocation.
    #[inline]
    pub const fn pick(self, fr: &'static str, en: &'static str) -> &'static str {
        match self {
            Self::Fr => fr,
            Self::En => en,
        }
    }
}

/// Timezone offsets supported in Settings -> Date & Time (Spec §1.14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimezonePreset {
    pub name: &'static str,
    pub offset_sec: i64,
}

pub const TIMEZONES: [TimezonePreset; 5] = [
    TimezonePreset { name: "Africa/Abidjan (UTC+0)", offset_sec: 0 },
    TimezonePreset { name: "UTC (UTC+0)", offset_sec: 0 },
    TimezonePreset { name: "Europe/Paris (UTC+1)", offset_sec: 3600 },
    TimezonePreset { name: "America/New_York (UTC-5)", offset_sec: -18000 },
    TimezonePreset { name: "Asia/Tokyo (UTC+9)", offset_sec: 32400 },
];
