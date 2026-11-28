//! RTC clock, timezone conversion, bilingual formatting, and month calendar calculation.

use crate::i18n::{Lang, TIMEZONES};

#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
pub struct RtcClock {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub weekday: u32, // 0=Sun .. 6=Sat
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub uptime_sec: u64,
}

impl RtcClock {
    pub fn format_time(&self, use_24h: bool) -> String {
        if use_24h {
            format!("{:02}:{:02}", self.hour, self.minute)
        } else {
            let suffix = if self.hour >= 12 { "PM" } else { "AM" };
            let h12 = match self.hour % 12 {
                0 => 12,
                h => h,
            };
            format!("{:02}:{:02} {}", h12, self.minute, suffix)
        }
    }

    pub fn format_time_sec(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    pub fn format_date(&self, lang: Lang) -> String {
        const DAYS_FR: [&str; 7] = ["Dim", "Lun", "Mar", "Mer", "Jeu", "Ven", "Sam"];
        const DAYS_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS_FR: [&str; 12] = [
            "Jan", "Fév", "Mar", "Avr", "Mai", "Juin", "Juil", "Août", "Sep", "Oct", "Nov", "Déc",
        ];
        const MONTHS_EN: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let wd = if lang.is_en() {
            DAYS_EN[(self.weekday as usize) % 7]
        } else {
            DAYS_FR[(self.weekday as usize) % 7]
        };
        let mo = if lang.is_en() {
            MONTHS_EN[(self.month.saturating_sub(1) as usize) % 12]
        } else {
            MONTHS_FR[(self.month.saturating_sub(1) as usize) % 12]
        };
        format!("{} {} {} {}", wd, self.day, mo, self.year)
    }

    pub fn month_title(&self, lang: Lang) -> String {
        const FULL_FR: [&str; 12] = [
            "Janvier", "Février", "Mars", "Avril", "Mai", "Juin",
            "Juillet", "Août", "Septembre", "Octobre", "Novembre", "Décembre",
        ];
        const FULL_EN: [&str; 12] = [
            "January", "February", "March", "April", "May", "June",
            "July", "August", "September", "October", "November", "December",
        ];
        let idx = (self.month.saturating_sub(1) as usize) % 12;
        format!("{} {}", if lang.is_en() { FULL_EN[idx] } else { FULL_FR[idx] }, self.year)
    }

    /// Number of days in the current month + weekday index of day 1 (`0=Mon .. 6=Sun`).
    pub fn month_grid_info(&self) -> (u32, u32) {
        let leap = (self.year % 4 == 0 && self.year % 100 != 0) || self.year % 400 == 0;
        let days = match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => if leap { 29 } else { 28 },
            _ => 30,
        };
        // Monday-based weekday of today: Sun(0) -> 6, Mon(1) -> 0
        let mon_today = (self.weekday + 6) % 7;
        let offset = (self.day.saturating_sub(1)) % 7;
        let first_mon_wd = (mon_today + 7 - offset) % 7;
        (days, first_mon_wd)
    }
}

pub fn unix_to_rtc(unix_sec: i64, tz_idx: usize, uptime: u64) -> RtcClock {
    let tz_off = TIMEZONES.get(tz_idx).map(|t| t.offset_sec).unwrap_or(0);
    let secs = unix_sec + tz_off;
    let days_since_epoch = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    let weekday = ((days_since_epoch + 4).rem_euclid(7)) as u32;
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (y + if month <= 2 { 1 } else { 0 }) as i32;
    RtcClock {
        year,
        month,
        day,
        weekday,
        hour,
        minute,
        second,
        uptime_sec: uptime,
    }
}
