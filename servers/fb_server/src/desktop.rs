#![allow(dead_code)]

//! # `desktop.rs` — ExoCompositor Modern Plasma / Kickoff Desktop (Wayland / Strata)
//!
//! Renders the native Exo-OS Full-HD desktop into the RAM shadow backbuffer:
//! 1. High-resolution Bayer-dithered RGB565 wallpaper (`wallpaper_640x348_rgb565.bin`) scaled 3:1
//!    to `1920x1044` with fixed-point bilinear interpolation.
//! 2. Interactive `xdg_toplevel` Wayland windows (`ExoTerm - exosh (Wayland)` terminal
//!    and client SHM surfaces) composited in painter z-order with traffic-light controls & move/resize.
//! 3. Native Exo-OS Kickoff Application Launcher Menu (zero legacy external branding).
//! 4. Full-width bottom Plasma panel (`DOCK_BAR_H = 36`) with launcher emblem, quick icons,
//!    interactive window task pills (clickable to minimize/restore), live clipboard badge, and live UTC clock.
//! 5. Live CMOS RTC Calendar & system HUD cards (`ExoDoctor`, `ExoFS Storage`, `Wayland`, `About Exo-OS`).

use crate::shared_font::{glyph_for, FONT_GLYPH_HEIGHT, FONT_GLYPH_WIDTH};
use crate::surface_table::{SurfaceTable as WaylandState, MAX_SHM_SURFACES};
use crate::Framebuffer;
use exo_syscall_abi as syscall;

static WALLPAPER_RGB565: &[u8] =
    include_bytes!("../../../assets/wallpapers/wallpaper_640x348_rgb565.bin");
const WP_SRC_W: u32 = 640;
const WP_SRC_H: u32 = 348;

pub const TOP_BAR_H: u32 = 0;
pub const WIN_TOP_Y: u32 = 64;
pub const WIN_TITLE_H: u32 = 24;
pub const TERM_TOP_Y: u32 = WIN_TOP_Y + WIN_TITLE_H + 6;
pub const WIN_LEFT_X: u32 = 500;
pub const TERM_LEFT_X: u32 = WIN_LEFT_X + 10;
pub const WIN_RIGHT_MARGIN: u32 = 14;
pub const TERM_RIGHT_MARGIN: u32 = WIN_RIGHT_MARGIN + 8;
pub const DOCK_BAR_H: u32 = 36;
pub const WIN_BOTTOM_MARGIN: u32 = DOCK_BAR_H + 6;
pub const HEADER_TOTAL_H: u32 = TERM_TOP_Y;

pub const KICKOFF_X: u32 = 8;
pub const KICKOFF_W: u32 = 460;
pub const KICKOFF_H: u32 = 520;
pub const KICKOFF_SIDEBAR_W: u32 = 154;
pub const KICKOFF_CAT_Y_OFFSET: u32 = 8;
pub const KICKOFF_CAT_PITCH: u32 = 38;

const MICRO_W: u32 = 5;
const MICRO_H: u32 = 7;
const MICRO_ADV: u32 = 6;

const HUD_W: u32 = 348;
const HUD_H: u32 = 224;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DockOverlay {
    None = 0,
    Doctor = 1,
    Calendar = 2,
    Storage = 3,
    Wayland = 4,
    About = 5,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KickoffAction {
    OpenTerminal,
    RunCmd(&'static [u8]),
    ToggleOverlay(DockOverlay),
    ClearTerminal,
    CloseMenu,
    Reboot,
    Shutdown,
}

#[derive(Clone, Copy)]
pub struct KickoffItem {
    pub title: &'static [u8],
    pub subtitle: &'static [u8],
    pub icon: u8,
    pub action: KickoffAction,
}

#[derive(Clone, Copy)]
pub struct RtcDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub days_epoch: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

pub const KICKOFF_CATEGORIES: [&[u8]; 8] = [
    b"Favorites",
    b"All Applications",
    b"System & Security",
    b"Wayland & GUI",
    b"Development",
    b"Network & IPC",
    b"Storage & FS",
    b"Utilities",
];

static CAT_FAVORITES: [KickoffItem; 8] = [
    KickoffItem {
        title: b"ExoTerm (exosh)",
        subtitle: b"Wayland native interactive shell",
        icon: 0,
        action: KickoffAction::OpenTerminal,
    },
    KickoffItem {
        title: b"ExoDoctor Health",
        subtitle: b"Kernel, ExoShield & 19-service audit",
        icon: 6,
        action: KickoffAction::RunCmd(b"exo doctor\n"),
    },
    KickoffItem {
        title: b"Wayland Demo",
        subtitle: b"wayland-rs SHM & xdg_toplevel test",
        icon: 5,
        action: KickoffAction::RunCmd(b"exo wayland\n"),
    },
    KickoffItem {
        title: b"ExoFS Explorer",
        subtitle: b"Browse ExoFS v2 root & Storage HUD",
        icon: 3,
        action: KickoffAction::RunCmd(b"ls -la /\n"),
    },
    KickoffItem {
        title: b"Process & Task Top",
        subtitle: b"Inspect Ring 0/1 tasks & CPU ticks",
        icon: 1,
        action: KickoffAction::RunCmd(b"top\n"),
    },
    KickoffItem {
        title: b"Calendar & RTC",
        subtitle: b"Live CMOS UTC clock & calendar HUD",
        icon: 4,
        action: KickoffAction::ToggleOverlay(DockOverlay::Calendar),
    },
    KickoffItem {
        title: b"System Telemetry",
        subtitle: b"Display CPU, memory & Wayland stack",
        icon: 2,
        action: KickoffAction::RunCmd(b"exo info\n"),
    },
    KickoffItem {
        title: b"About Exo-OS",
        subtitle: b"Strata microkernel desktop overview",
        icon: 2,
        action: KickoffAction::ToggleOverlay(DockOverlay::About),
    },
];

static CAT_ALL_APPS: [KickoffItem; 8] = [
    KickoffItem {
        title: b"ExoTerm (exosh)",
        subtitle: b"Wayland native interactive shell",
        icon: 0,
        action: KickoffAction::OpenTerminal,
    },
    KickoffItem {
        title: b"ExoDoctor Diagnostics",
        subtitle: b"Live microkernel health verification",
        icon: 6,
        action: KickoffAction::RunCmd(b"exo doctor\n"),
    },
    KickoffItem {
        title: b"ExoShield NGAV Audit",
        subtitle: b"Inspect 67-edge policy & MAC rules",
        icon: 6,
        action: KickoffAction::RunCmd(b"exo shield\n"),
    },
    KickoffItem {
        title: b"ExoFS v2 Browser",
        subtitle: b"Storage HUD & BLAKE3/Ed25519 cache",
        icon: 3,
        action: KickoffAction::ToggleOverlay(DockOverlay::Storage),
    },
    KickoffItem {
        title: b"Wayland Compositor",
        subtitle: b"Wire v1.22 SHM & surface inspector",
        icon: 5,
        action: KickoffAction::ToggleOverlay(DockOverlay::Wayland),
    },
    KickoffItem {
        title: b"Calendar CLI & HUD",
        subtitle: b"Print live Gregorian UTC calendar",
        icon: 4,
        action: KickoffAction::RunCmd(b"calendar\n"),
    },
    KickoffItem {
        title: b"IPC Router Bench",
        subtitle: b"Measure 1000 zero-copy IPC heartbeats",
        icon: 1,
        action: KickoffAction::RunCmd(b"bench ipc 1000\n"),
    },
    KickoffItem {
        title: b"Loopback Ping",
        subtitle: b"ICMP echo test via network_server",
        icon: 5,
        action: KickoffAction::RunCmd(b"ping 127.0.0.1\n"),
    },
];

static CAT_SYSTEM: [KickoffItem; 7] = [
    KickoffItem {
        title: b"ExoDoctor HUD",
        subtitle: b"Open Ring 0 / Ring 1 status card",
        icon: 6,
        action: KickoffAction::ToggleOverlay(DockOverlay::Doctor),
    },
    KickoffItem {
        title: b"ExoShield Audit",
        subtitle: b"Zero-trust Ring 1 capability report",
        icon: 6,
        action: KickoffAction::RunCmd(b"exo shield\n"),
    },
    KickoffItem {
        title: b"Memory Info",
        subtitle: b"Frame allocator, heaps & slab pages",
        icon: 1,
        action: KickoffAction::RunCmd(b"meminfo\n"),
    },
    KickoffItem {
        title: b"Syscall Telemetry",
        subtitle: b"Kernel syscall invocation counters",
        icon: 1,
        action: KickoffAction::RunCmd(b"syscall-stat\n"),
    },
    KickoffItem {
        title: b"Process List (ps)",
        subtitle: b"Active tasks, UID/GID & memory",
        icon: 1,
        action: KickoffAction::RunCmd(b"ps\n"),
    },
    KickoffItem {
        title: b"Uname & Build",
        subtitle: b"Kernel architecture & version info",
        icon: 2,
        action: KickoffAction::RunCmd(b"uname -a\n"),
    },
    KickoffItem {
        title: b"About Exo-OS",
        subtitle: b"Show Strata desktop specifications",
        icon: 2,
        action: KickoffAction::ToggleOverlay(DockOverlay::About),
    },
];

static CAT_WAYLAND: [KickoffItem; 6] = [
    KickoffItem {
        title: b"Focus ExoTerm Window",
        subtitle: b"Restore & focus Wayland terminal",
        icon: 0,
        action: KickoffAction::OpenTerminal,
    },
    KickoffItem {
        title: b"Wayland Inspector HUD",
        subtitle: b"Monitor active SHM & xdg surfaces",
        icon: 5,
        action: KickoffAction::ToggleOverlay(DockOverlay::Wayland),
    },
    KickoffItem {
        title: b"Run wayland-rs Roundtrip",
        subtitle: b"memfd + SCM_RIGHTS + ARGB8888 test",
        icon: 5,
        action: KickoffAction::RunCmd(b"exo wayland\n"),
    },
    KickoffItem {
        title: b"Clear ExoTerm Console",
        subtitle: b"Reset terminal cell viewport",
        icon: 1,
        action: KickoffAction::ClearTerminal,
    },
    KickoffItem {
        title: b"Framebuffer Resolution",
        subtitle: b"Query active Full-HD GOP display",
        icon: 2,
        action: KickoffAction::RunCmd(b"exo fb\n"),
    },
    KickoffItem {
        title: b"Close Launcher Menu",
        subtitle: b"Reveal full desktop wallpaper",
        icon: 4,
        action: KickoffAction::CloseMenu,
    },
];

static CAT_DEV: [KickoffItem; 6] = [
    KickoffItem {
        title: b"ExoShell (exosh)",
        subtitle: b"Open interactive command shell",
        icon: 0,
        action: KickoffAction::OpenTerminal,
    },
    KickoffItem {
        title: b"Bench IPC (1000 ops)",
        subtitle: b"Roundtrip latency in ns/op",
        icon: 1,
        action: KickoffAction::RunCmd(b"bench ipc 1000\n"),
    },
    KickoffItem {
        title: b"Bench Scheduler (5000)",
        subtitle: b"Cooperative context switch timing",
        icon: 1,
        action: KickoffAction::RunCmd(b"bench sched 5000\n"),
    },
    KickoffItem {
        title: b"Bench Crypto (200 ops)",
        subtitle: b"RING1 CSPRNG & AEAD benchmark",
        icon: 6,
        action: KickoffAction::RunCmd(b"bench crypto 200\n"),
    },
    KickoffItem {
        title: b"Bench Filesystem (1 MiB)",
        subtitle: b"Sequential ExoFS write/read rate",
        icon: 3,
        action: KickoffAction::RunCmd(b"bench fs 1\n"),
    },
    KickoffItem {
        title: b"Shell Builtins Help",
        subtitle: b"List all exosh commands",
        icon: 2,
        action: KickoffAction::RunCmd(b"help\n"),
    },
];

static CAT_NETWORK: [KickoffItem; 5] = [
    KickoffItem {
        title: b"Ping Loopback",
        subtitle: b"Send 4 ICMP echo probes (Ctrl+C ok)",
        icon: 5,
        action: KickoffAction::RunCmd(b"ping 127.0.0.1\n"),
    },
    KickoffItem {
        title: b"Exo Network Check",
        subtitle: b"Verify TCP/UDP & loopback drivers",
        icon: 5,
        action: KickoffAction::RunCmd(b"exo net\n"),
    },
    KickoffItem {
        title: b"IPC Router Stats",
        subtitle: b"Endpoint queues & ring buffer load",
        icon: 1,
        action: KickoffAction::RunCmd(b"ipc-stat\n"),
    },
    KickoffItem {
        title: b"TCP Port Probe :80",
        subtitle: b"Test TCP connect handshake flow",
        icon: 5,
        action: KickoffAction::RunCmd(b"tcping 127.0.0.1 80\n"),
    },
    KickoffItem {
        title: b"Network Interfaces",
        subtitle: b"Inspect socket stack configuration",
        icon: 2,
        action: KickoffAction::RunCmd(b"exo info\n"),
    },
];

static CAT_STORAGE: [KickoffItem; 6] = [
    KickoffItem {
        title: b"ExoFS v2 Storage HUD",
        subtitle: b"Open live VFS & SHM status card",
        icon: 3,
        action: KickoffAction::ToggleOverlay(DockOverlay::Storage),
    },
    KickoffItem {
        title: b"List Root /",
        subtitle: b"Inspect root directory entries",
        icon: 3,
        action: KickoffAction::RunCmd(b"ls -la /\n"),
    },
    KickoffItem {
        title: b"Filesystem Usage (df)",
        subtitle: b"Block & inode utilization summary",
        icon: 3,
        action: KickoffAction::RunCmd(b"df\n"),
    },
    KickoffItem {
        title: b"Sync Filesystem",
        subtitle: b"Flush writeback buffers to disk",
        icon: 3,
        action: KickoffAction::RunCmd(b"sync\n"),
    },
    KickoffItem {
        title: b"Browse /sbin Services",
        subtitle: b"List Ring 1 microkernel servers",
        icon: 1,
        action: KickoffAction::RunCmd(b"ls /sbin\n"),
    },
    KickoffItem {
        title: b"Browse /bin Utilities",
        subtitle: b"List coreutils & system binaries",
        icon: 1,
        action: KickoffAction::RunCmd(b"ls /bin\n"),
    },
];

static CAT_UTILITIES: [KickoffItem; 6] = [
    KickoffItem {
        title: b"Calendar & Clock HUD",
        subtitle: b"Live CMOS RTC month grid overlay",
        icon: 4,
        action: KickoffAction::ToggleOverlay(DockOverlay::Calendar),
    },
    KickoffItem {
        title: b"Calendar CLI View",
        subtitle: b"Print UTC calendar in ExoTerm",
        icon: 4,
        action: KickoffAction::RunCmd(b"calendar\n"),
    },
    KickoffItem {
        title: b"Audio Chime Test",
        subtitle: b"Play AC97 / PC speaker test tone",
        icon: 2,
        action: KickoffAction::RunCmd(b"chime\n"),
    },
    KickoffItem {
        title: b"Clear Console",
        subtitle: b"Clear ExoTerm scroll viewport",
        icon: 0,
        action: KickoffAction::ClearTerminal,
    },
    KickoffItem {
        title: b"Sleep 5s (Ctrl+C Test)",
        subtitle: b"Interruptible wait test (press Ctrl+C)",
        icon: 1,
        action: KickoffAction::RunCmd(b"sleep 5000\n"),
    },
    KickoffItem {
        title: b"About Exo-OS",
        subtitle: b"Build & microkernel highlights",
        icon: 2,
        action: KickoffAction::ToggleOverlay(DockOverlay::About),
    },
];

pub fn kickoff_items_for_category(cat: u8) -> &'static [KickoffItem] {
    match cat {
        0 => &CAT_FAVORITES,
        1 => &CAT_ALL_APPS,
        2 => &CAT_SYSTEM,
        3 => &CAT_WAYLAND,
        4 => &CAT_DEV,
        5 => &CAT_NETWORK,
        6 => &CAT_STORAGE,
        _ => &CAT_UTILITIES,
    }
}

/// Returns `(mx, my, mw, mh, sidebar_w, cat_pitch, row_h)` adapted to viewport resolution.
pub fn kickoff_layout(fb: Framebuffer) -> (u32, u32, u32, u32, u32, u32, u32) {
    let dock_y = fb.height.saturating_sub(DOCK_BAR_H);
    let wide = fb.width >= 1280 && fb.height >= 720;
    let mw = if wide {
        KICKOFF_W
    } else {
        360u32.min(fb.width.saturating_sub(16))
    };
    let mh = if wide {
        KICKOFF_H
    } else {
        444u32.min(dock_y.saturating_sub(12))
    };
    let mx = if wide { KICKOFF_X } else { 6 };
    let my = dock_y.saturating_sub(mh + 4);
    let sidebar_w = if wide { KICKOFF_SIDEBAR_W } else { 130 };
    let cat_pitch = if wide { KICKOFF_CAT_PITCH } else { 32 };
    let row_h = if wide { 48 } else { 38 };
    (mx, my, mw, mh, sidebar_w, cat_pitch, row_h)
}

#[cfg(target_os = "none")]
pub fn current_rtc_unix_secs() -> i64 {
    let mut ts = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_CLOCK_GETTIME,
            0, // CLOCK_REALTIME
            &mut ts as *mut Timespec as u64,
        )
    };
    if rc == 0 && ts.tv_sec > 0 {
        ts.tv_sec
    } else {
        // Compile-time safe floor if CMOS read unavailable
        1_794_744_000
    }
}

#[cfg(not(target_os = "none"))]
pub fn current_rtc_unix_secs() -> i64 {
    1_794_744_000
}

pub fn current_rtc_minute() -> i64 {
    current_rtc_unix_secs().div_euclid(60)
}

pub fn rtc_now_utc() -> RtcDateTime {
    let secs = current_rtc_unix_secs();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    let (year, month, day) = civil_from_days(days);
    RtcDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        days_epoch: days,
    }
}

/// Howard Hinnant's civil-from-days algorithm (UTC proleptic Gregorian).
pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}

fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn weekday_sun0_of_day(days_epoch: i64) -> u32 {
    // 1970-01-01 was Thursday (4)
    ((days_epoch + 4).rem_euclid(7)) as u32
}

fn month_short(m: u32) -> &'static [u8] {
    match m {
        1 => b"Jan",
        2 => b"Feb",
        3 => b"Mar",
        4 => b"Apr",
        5 => b"May",
        6 => b"Jun",
        7 => b"Jul",
        8 => b"Aug",
        9 => b"Sep",
        10 => b"Oct",
        11 => b"Nov",
        12 => b"Dec",
        _ => b"---",
    }
}

fn month_full(m: u32) -> &'static [u8] {
    match m {
        1 => b"January",
        2 => b"February",
        3 => b"March",
        4 => b"April",
        5 => b"May",
        6 => b"June",
        7 => b"July",
        8 => b"August",
        9 => b"September",
        10 => b"October",
        11 => b"November",
        12 => b"December",
        _ => b"Unknown",
    }
}

fn write_2digits(out: &mut [u8], pos: usize, val: u32) {
    if pos + 2 <= out.len() {
        out[pos] = b'0' + ((val / 10) % 10) as u8;
        out[pos + 1] = b'0' + (val % 10) as u8;
    }
}

fn format_clock_hm(dt: &RtcDateTime, out: &mut [u8; 9]) -> usize {
    // "HH:MM UTC"
    write_2digits(out, 0, dt.hour);
    out[2] = b':';
    write_2digits(out, 3, dt.minute);
    out[5] = b' ';
    out[6] = b'U';
    out[7] = b'T';
    out[8] = b'C';
    9
}

fn format_clock_hms(dt: &RtcDateTime, out: &mut [u8; 12]) -> usize {
    write_2digits(out, 0, dt.hour);
    out[2] = b':';
    write_2digits(out, 3, dt.minute);
    out[5] = b':';
    write_2digits(out, 6, dt.second);
    out[8..12].copy_from_slice(b" UTC");
    12
}

fn format_date_line(dt: &RtcDateTime, out: &mut [u8; 11]) -> usize {
    // "DD Mon YYYY"
    write_2digits(out, 0, dt.day);
    out[2] = b' ';
    let mon = month_short(dt.month);
    out[3..6].copy_from_slice(&mon[..3]);
    out[6] = b' ';
    let y = dt.year.clamp(0, 9999) as u32;
    out[7] = b'0' + ((y / 1000) % 10) as u8;
    out[8] = b'0' + ((y / 100) % 10) as u8;
    out[9] = b'0' + ((y / 10) % 10) as u8;
    out[10] = b'0' + (y % 10) as u8;
    11
}

pub struct DesktopState {
    pub active_overlay: DockOverlay,
    pub gpu_active: bool,
    pub frame_counter: u64,
    pub wayland_wire_count: u64,
    pub wayland_surfaces_active: u32,
    pub clipboard_count: usize,
}

impl DesktopState {
    pub const fn new() -> Self {
        Self {
            active_overlay: DockOverlay::None,
            gpu_active: true,
            frame_counter: 0,
            wayland_wire_count: 8,
            wayland_surfaces_active: 1,
            clipboard_count: 0,
        }
    }

    pub fn set_wayland_stats(&mut self, wire_count: u64, active_surfaces: u32) {
        self.wayland_wire_count = wire_count.max(8);
        self.wayland_surfaces_active = active_surfaces.max(1);
    }

    pub fn set_clipboard_count(&mut self, count: usize) {
        self.clipboard_count = count;
    }

    pub fn toggle_overlay(&mut self, kind: DockOverlay) {
        self.active_overlay = if self.active_overlay == kind {
            DockOverlay::None
        } else {
            kind
        };
    }

    pub fn hud_rect(fb: Framebuffer) -> (u32, u32, u32, u32) {
        let hx = fb.width.saturating_sub(HUD_W + WIN_RIGHT_MARGIN + 4);
        let hy = 18u32;
        (hx, hy, HUD_W, HUD_H)
    }

    /// Handles clicks on HUD overlay close button, HUD line selection copy, and panel overlay triggers.
    /// Returns `(handled, optional_copied_text)`.
    pub fn handle_mouse_click_with_copy(
        &mut self,
        mx: u32,
        my: u32,
        fb: Framebuffer,
    ) -> (bool, Option<&'static [u8]>) {
        if !fb.is_present() {
            return (false, None);
        }

        if self.active_overlay != DockOverlay::None {
            let (hx, hy, hw, hh) = Self::hud_rect(fb);
            if mx >= hx && mx < hx + hw && my >= hy && my < hy + hh {
                // Close button in top-right of HUD titlebar
                if mx >= hx + hw - 26 && my < hy + 26 {
                    self.active_overlay = DockOverlay::None;
                    return (true, None);
                }
                // Clicking inside HUD body copies diagnostic summary line!
                if my >= hy + 28 {
                    let line_idx = (my - (hy + 28)) / 22;
                    let copied = self.overlay_copy_line(line_idx);
                    return (true, copied);
                }
                return (true, None);
            }
        }

        let dock_y = fb.height.saturating_sub(DOCK_BAR_H);
        if my >= dock_y {
            if mx >= 76 && mx < 106 {
                self.toggle_overlay(DockOverlay::Storage);
                return (true, None);
            }
            if mx >= 108 && mx < 138 {
                self.toggle_overlay(DockOverlay::Wayland);
                return (true, None);
            }
            if mx >= 140 && mx < 170 {
                self.toggle_overlay(DockOverlay::Doctor);
                return (true, None);
            }
            if mx >= fb.width.saturating_sub(110) {
                self.toggle_overlay(DockOverlay::Calendar);
                return (true, None);
            }
        }
        (false, None)
    }

    pub fn handle_mouse_click(&mut self, mx: u32, my: u32, fb: Framebuffer) -> bool {
        self.handle_mouse_click_with_copy(mx, my, fb).0
    }

    fn overlay_copy_line(&self, line_idx: u32) -> Option<&'static [u8]> {
        match self.active_overlay {
            DockOverlay::Doctor => match line_idx {
                0 => Some(b"exo doctor"),
                1 => Some(b"exo shield"),
                2 => Some(b"uname -a"),
                3 => Some(b"ps"),
                _ => Some(b"exo info"),
            },
            DockOverlay::Storage => match line_idx {
                0 => Some(b"df"),
                1 => Some(b"ls -la /"),
                2 => Some(b"sync"),
                _ => Some(b"meminfo"),
            },
            DockOverlay::Wayland => match line_idx {
                0 | 1 | 2 => Some(b"exo wayland"),
                _ => Some(b"exo fb"),
            },
            DockOverlay::Calendar => Some(b"calendar"),
            DockOverlay::About => Some(b"exo info"),
            DockOverlay::None => None,
        }
    }

    pub fn render_active_overlay(&self, fb: Framebuffer) {
        if self.active_overlay != DockOverlay::None {
            self.render_overlay_hud(fb);
        }
    }

    fn render_overlay_hud(&self, fb: Framebuffer) {
        let (hx, hy, hud_w, hud_h) = Self::hud_rect(fb);
        let desktop_h = fb.height.saturating_sub(DOCK_BAR_H);
        if hy + hud_h > desktop_h {
            return;
        }

        let (title, accent_rgb): (&[u8], (u8, u8, u8)) = match self.active_overlay {
            DockOverlay::Doctor => (b"ExoDoctor & Shield", (0x38, 0xd7, 0x7c)),
            DockOverlay::Calendar => (b"Calendar & CMOS RTC", (0xff, 0xb7, 0x03)),
            DockOverlay::Storage => (b"ExoFS v2 Storage", (0x3d, 0xae, 0xe9)),
            DockOverlay::Wayland => (b"ExoCompositor Wayland", (0x00, 0xd4, 0xb2)),
            DockOverlay::About | DockOverlay::None => (b"About Exo-OS Strata", (0x5c, 0xb8, 0xff)),
        };

        blend_rect_shadow(fb, hx + 4, hy + 4, hud_w, hud_h, 0x04, 0x06, 0x0e, 155);
        fb.fill_rect_shadow(hx, hy, hud_w, hud_h, fb.encode(0x3c, 0x45, 0x5e));
        fb.fill_rect_shadow(hx + 1, hy + 1, hud_w - 2, hud_h - 2, fb.encode(0x18, 0x1c, 0x28));
        fb.fill_rect_shadow(hx + 1, hy + 1, hud_w - 2, 24, fb.encode(0x22, 0x27, 0x36));
        fb.fill_rect_shadow(
            hx + 1,
            hy + 25,
            hud_w - 2,
            2,
            fb.encode(accent_rgb.0, accent_rgb.1, accent_rgb.2),
        );
        draw_text_alpha_7px(fb, hx + 10, hy + 5, hx + hud_w - 28, title, (0xf0, 0xf5, 0xff));
        // Close button `[X]`
        fb.fill_rect_shadow(hx + hud_w - 22, hy + 6, 14, 14, fb.encode(0x3a, 0x42, 0x56));
        draw_text_micro(fb, hx + hud_w - 17, hy + 10, b"X", (0xf2, 0xf6, 0xff));

        let tx = hx + 12;
        let ty = hy + 36;
        let max_x = hx + hud_w - 10;
        match self.active_overlay {
            DockOverlay::Doctor => {
                draw_text_alpha_7px(fb, tx, ty, max_x, b"Kernel : Microkernel Ring 0", (0xd6, 0xe2, 0xf2));
                draw_text_alpha_7px(fb, tx, ty + 24, max_x, b"Shield : ExoShield 67-Edge OK", (0x60, 0xff, 0x90));
                draw_text_alpha_7px(fb, tx, ty + 48, max_x, b"Boot   : ExoPhoenix A/B", (0x62, 0xe6, 0xff));
                draw_text_alpha_7px(fb, tx, ty + 72, max_x, b"Ring 1 : 19 Services Online", (0x60, 0xff, 0x90));
                draw_text_alpha_7px(fb, tx, ty + 96, max_x, b"Input  : PS/2 + Mouse Drag OK", (0xff, 0xd1, 0x66));
                draw_text_micro(fb, tx, ty + 132, b"Click a row to copy CLI command to clipboard", (0x7e, 0x8e, 0xae));
            }
            DockOverlay::Calendar => {
                let dt = rtc_now_utc();
                let mut hdr = [b' '; 32];
                let m_name = month_full(dt.month);
                let ml = m_name.len().min(12);
                hdr[..ml].copy_from_slice(&m_name[..ml]);
                hdr[ml] = b' ';
                let y = dt.year.clamp(0, 9999) as u32;
                hdr[ml + 1] = b'0' + ((y / 1000) % 10) as u8;
                hdr[ml + 2] = b'0' + ((y / 100) % 10) as u8;
                hdr[ml + 3] = b'0' + ((y / 10) % 10) as u8;
                hdr[ml + 4] = b'0' + (y % 10) as u8;
                hdr[ml + 5..ml + 8].copy_from_slice(b" - ");
                let mut tbuf = [b' '; 12];
                let _ = format_clock_hms(&dt, &mut tbuf);
                let hdr_len = (ml + 8 + 12).min(hdr.len());
                hdr[ml + 8..hdr_len].copy_from_slice(&tbuf[..(hdr_len - (ml + 8))]);
                draw_text_alpha_7px(fb, tx, ty, max_x, &hdr[..hdr_len], (0xff, 0xd1, 0x66));
                draw_text_alpha_7px(fb, tx, ty + 24, max_x, b"Su  Mo  Tu  We  Th  Fr  Sa", (0x7b, 0xa8, 0xff));

                let first_wd = weekday_sun0_of_day(dt.days_epoch - (dt.day as i64 - 1));
                let dim = days_in_month(dt.year, dt.month);
                let mut week = 0u32;
                let mut cur_day = 1u32;
                while week < 6 && cur_day <= dim {
                    let row_y = ty + 48 + week * 20;
                    let mut wd = 0u32;
                    while wd < 7 && cur_day <= dim {
                        if week == 0 && wd < first_wd {
                            wd += 1;
                            continue;
                        }
                        let cell_x = tx + wd * 32;
                        let mut d_txt = [b' '; 4];
                        let is_today = cur_day == dt.day;
                        if is_today {
                            fb.fill_rect_shadow(cell_x.saturating_sub(2), row_y.saturating_sub(1), 28, 16, fb.encode(0x20, 0x58, 0x42));
                            d_txt[0] = b'[';
                            d_txt[1] = if cur_day >= 10 { b'0' + (cur_day / 10) as u8 } else { b' ' };
                            d_txt[2] = b'0' + (cur_day % 10) as u8;
                            d_txt[3] = b']';
                            draw_text_micro(fb, cell_x, row_y + 4, &d_txt, (0x60, 0xff, 0x90));
                        } else {
                            d_txt[0] = b' ';
                            d_txt[1] = if cur_day >= 10 { b'0' + (cur_day / 10) as u8 } else { b' ' };
                            d_txt[2] = b'0' + (cur_day % 10) as u8;
                            draw_text_micro(fb, cell_x, row_y + 4, &d_txt[..3], (0xe2, 0xea, 0xf6));
                        }
                        cur_day += 1;
                        wd += 1;
                    }
                    week += 1;
                }
            }
            DockOverlay::Storage => {
                draw_text_alpha_7px(fb, tx, ty, max_x, b"Root / : ExoFS v2 Read-Write", (0x62, 0xe6, 0xff));
                draw_text_alpha_7px(fb, tx, ty + 24, max_x, b"Crypto : BLAKE3 + Ed25519", (0xe2, 0xea, 0xf6));
                draw_text_alpha_7px(fb, tx, ty + 48, max_x, b"Sync   : Writeback kthread OK", (0x60, 0xff, 0x90));
                draw_text_alpha_7px(fb, tx, ty + 72, max_x, b"SHM    : memfd + MAP_SHARED", (0xff, 0xd1, 0x66));
                draw_text_micro(fb, tx, ty + 124, b"Click a row to copy command ('df' / 'ls')", (0x7e, 0x8e, 0xae));
            }
            DockOverlay::Wayland => {
                draw_text_alpha_7px(fb, tx, ty, max_x, b"Wayland Wire Protocol v1.22", (0x00, 0xf5, 0xd4));
                draw_text_alpha_7px(fb, tx, ty + 24, max_x, b"Formats: ARGB/XRGB/ABGR/565", (0xe2, 0xea, 0xf6));
                draw_text_alpha_7px(fb, tx, ty + 48, max_x, b"Socket : AF_UNIX + SCM_RIGHTS", (0x60, 0xff, 0x90));
                let mut stat_line = [b' '; 28];
                stat_line[..15].copy_from_slice(b"Surfaces:   | #");
                stat_line[10] = b'0' + (self.wayland_surfaces_active.min(9) as u8);
                write_u64_ascii(&mut stat_line[15..], self.wayland_wire_count);
                draw_text_alpha_7px(fb, tx, ty + 72, max_x, &stat_line, (0xff, 0xd1, 0x66));
                draw_text_micro(fb, tx, ty + 124, b"Drag titlebar to move | Right-click to paste", (0x8c, 0xa2, 0xcc));
            }
            DockOverlay::About | DockOverlay::None => {
                draw_text_alpha_7px(fb, tx, ty, max_x, b"Exo-OS Strata Desktop", (0x62, 0xe6, 0xff));
                draw_text_alpha_7px(fb, tx, ty + 24, max_x, b"Target : x86_64-unknown-exo", (0xe2, 0xea, 0xf6));
                draw_text_alpha_7px(fb, tx, ty + 48, max_x, b"Display: Native Full-HD 32bpp", (0x60, 0xff, 0x90));
                draw_text_alpha_7px(fb, tx, ty + 72, max_x, b"GUI    : Wayland + ZeroCopy SHM", (0xff, 0xd1, 0x66));
                draw_text_alpha_7px(fb, tx, ty + 96, max_x, b"Shell  : exosh (Ctrl+C & Copy)", (0xe2, 0xea, 0xf6));
            }
        }
    }
}

pub fn render_desktop_base_with_wayland(fb: Framebuffer, wayland: &WaylandState) {
    if !fb.is_present() || fb.height < 240 || fb.width < 320 {
        return;
    }
    render_wallpaper(fb);
    wayland.composite_all_surfaces(fb);
}

pub fn render_desktop_with_wayland(fb: Framebuffer, wayland: &WaylandState) {
    if !fb.is_present() || fb.height < 240 || fb.width < 320 {
        return;
    }
    render_wallpaper(fb);
    wayland.composite_all_surfaces(fb);
    if wayland.kickoff_open && fb.height > DOCK_BAR_H + 260 && fb.width > 380 {
        render_kickoff_menu(fb, wayland.selected_category, wayland.selected_app);
    }
    render_bottom_panel_with_wayland(fb, wayland);
}

#[inline(always)]
fn decode_rgb565_idx(idx: usize) -> (u32, u32, u32) {
    let off = idx * 2;
    let v = (WALLPAPER_RGB565[off] as u32) | ((WALLPAPER_RGB565[off + 1] as u32) << 8);
    let r5 = (v >> 11) & 0x1F;
    let g6 = (v >> 5) & 0x3F;
    let b5 = v & 0x1F;
    let r = (r5 << 3) | (r5 >> 2);
    let g = (g6 << 2) | (g6 >> 4);
    let b = (b5 << 3) | (b5 >> 2);
    (r, g, b)
}

pub fn render_wallpaper_rect(fb: Framebuffer, rx: u32, ry: u32, rw: u32, rh: u32) {
    if !fb.is_present() || WALLPAPER_RGB565.len() < (WP_SRC_W * WP_SRC_H * 2) as usize {
        return;
    }
    let desktop_h = fb.height.saturating_sub(DOCK_BAR_H);
    let x0 = rx.min(fb.width);
    let y0 = ry.min(desktop_h);
    let x1 = rx.saturating_add(rw).min(fb.width);
    let y1 = ry.saturating_add(rh).min(desktop_h);
    if x0 >= x1 || y0 >= y1 || fb.width == 0 || desktop_h == 0 {
        return;
    }

    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();
    let is_bgr = fb.format == 1;

    let step_x: u64 = (((WP_SRC_W - 1) as u64) << 16) / (fb.width.saturating_sub(1).max(1) as u64);
    let step_y: u64 = (((WP_SRC_H - 1) as u64) << 16) / (desktop_h.saturating_sub(1).max(1) as u64);

    let mut y = y0;
    while y < y1 {
        let fy = ((y as u64) * step_y).min(((WP_SRC_H - 1) as u64) << 16);
        let ys0 = (fy >> 16).min((WP_SRC_H - 1) as u64) as usize;
        let ys1 = (ys0 + 1).min((WP_SRC_H - 1) as usize);
        let ty = ((fy >> 8) & 0xFF) as u32;
        let inv_ty = 256 - ty;

        let src_row0 = ys0 * (WP_SRC_W as usize);
        let src_row1 = ys1 * (WP_SRC_W as usize);
        let row_addr = base.saturating_add((y as usize).saturating_mul(stride));

        let mut fx = (x0 as u64) * step_x;
        let max_fx = ((WP_SRC_W - 1) as u64) << 16;
        let mut x = x0;
        while x < x1 {
            let clamped_fx = fx.min(max_fx);
            let xs0 = (clamped_fx >> 16) as usize;
            let xs1 = (xs0 + 1).min((WP_SRC_W - 1) as usize);
            let tx = ((clamped_fx >> 8) & 0xFF) as u32;
            let inv_tx = 256 - tx;

            let (r00, g00, b00) = decode_rgb565_idx(src_row0 + xs0);
            let (r10, g10, b10) = decode_rgb565_idx(src_row0 + xs1);
            let (r01, g01, b01) = decode_rgb565_idx(src_row1 + xs0);
            let (r11, g11, b11) = decode_rgb565_idx(src_row1 + xs1);

            let w00 = inv_tx * inv_ty;
            let w10 = tx * inv_ty;
            let w01 = inv_tx * ty;
            let w11 = tx * ty;

            let r = (r00 * w00 + r10 * w10 + r01 * w01 + r11 * w11) >> 16;
            let g = (g00 * w00 + g10 * w10 + g01 * w01 + g11 * w11) >> 16;
            let b = (b00 * w00 + b10 * w10 + b01 * w01 + b11 * w11) >> 16;

            let px = if is_bgr {
                (b << 16) | (g << 8) | r
            } else {
                (r << 16) | (g << 8) | b
            };
            write_shadow_px(row_addr, x, bpp, px);

            fx += step_x;
            x += 1;
        }
        y += 1;
    }
}

pub fn render_wallpaper(fb: Framebuffer) {
    let desktop_h = fb.height.saturating_sub(DOCK_BAR_H);
    render_wallpaper_rect(fb, 0, 0, fb.width, desktop_h);
}

pub fn render_terminal_window_at(
    fb: Framebuffer,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    title: &[u8],
    surface_id: u32,
    focused: bool,
    fill_interior: bool,
) {
    if !fb.is_present() || w < 120 || h < 80 {
        return;
    }

    let win_border = if focused {
        fb.encode(0x3d, 0xae, 0xe9)
    } else {
        fb.encode(0x38, 0x41, 0x59)
    };
    let title_bg = fb.encode(0x1d, 0x20, 0x2b);
    let div_col = fb.encode(0x2d, 0x33, 0x45);

    blend_rect_shadow(fb, x + w, y + 4, 5, h, 0x04, 0x05, 0x0b, 145);
    blend_rect_shadow(fb, x + 4, y + h, w.saturating_sub(4), 5, 0x04, 0x05, 0x0b, 145);

    fb.fill_rect_shadow(x, y, 2, h, win_border);
    fb.fill_rect_shadow(x + w - 2, y, 2, h, win_border);
    fb.fill_rect_shadow(x, y, w, 1, win_border);
    fb.fill_rect_shadow(x, y + h - 2, w, 2, win_border);

    fb.fill_rect_shadow(x + 2, y + 1, w - 4, WIN_TITLE_H - 2, title_bg);
    fb.fill_rect_shadow(x + 2, y + WIN_TITLE_H - 1, w - 4, 1, div_col);

    // Interactive Traffic-Light dots (Close, Minimize, Maximize)
    fb.fill_rect_shadow(x + 10, y + 8, 8, 8, fb.encode(0xff, 0x5f, 0x56));
    fb.fill_rect_shadow(x + 24, y + 8, 8, 8, fb.encode(0xff, 0xbd, 0x2e));
    fb.fill_rect_shadow(x + 38, y + 8, 8, 8, fb.encode(0x27, 0xc9, 0x3f));

    let effective_title = if title.is_empty() {
        b"ExoTerm - exosh (Wayland)"
    } else {
        title
    };
    let title_w = (effective_title.len() as u32) * 7;
    let title_x = (x + (w / 2).saturating_sub(title_w / 2)).max(x + 56);
    draw_text_alpha_7px(fb, title_x, y + 4, x + w - 58, effective_title, (0xd8, 0xe1, 0xf2));

    let mut badge = [b' '; 6];
    badge[..3].copy_from_slice(b"wl#");
    write_u64_ascii(&mut badge[3..], surface_id as u64);
    let badge_len = if surface_id >= 10 { 5 } else { 4 };
    draw_text_micro(fb, x + w - 42, y + 8, &badge[..badge_len], (0x3d, 0xae, 0xe9));

    if fill_interior {
        let term_bg = fb.encode(0x12, 0x14, 0x1d);
        fb.fill_rect_shadow(
            x + 2,
            y + WIN_TITLE_H,
            w - 4,
            h.saturating_sub(WIN_TITLE_H + 2),
            term_bg,
        );
    }

    // Bottom-right corner resize grip accent
    let grip = fb.encode(0x52, 0x62, 0x80);
    fb.fill_rect_shadow(x + w - 10, y + h - 5, 6, 1, grip);
    fb.fill_rect_shadow(x + w - 7, y + h - 8, 3, 1, grip);
}

pub fn render_wayland_window_chrome(
    fb: Framebuffer,
    wx: u32,
    wy: u32,
    content_w: u32,
    content_h: u32,
    title: &[u8],
    surface_id: u32,
    focused: bool,
) {
    let w = content_w + 4;
    let h = content_h + 28;
    let border = if focused {
        fb.encode(0x3d, 0xae, 0xe9)
    } else {
        fb.encode(0x38, 0x41, 0x59)
    };
    let title_bg = fb.encode(0x1d, 0x20, 0x2b);
    let body_bg = fb.encode(0x14, 0x17, 0x22);

    blend_rect_shadow(fb, wx + w, wy + 4, 4, h, 0x04, 0x05, 0x0b, 140);
    blend_rect_shadow(fb, wx + 4, wy + h, w.saturating_sub(4), 4, 0x04, 0x05, 0x0b, 140);

    fb.fill_rect_shadow(wx, wy, w, h, border);
    fb.fill_rect_shadow(wx + 2, wy + 1, content_w, 24, title_bg);
    fb.fill_rect_shadow(wx + 2, wy + 26, content_w, content_h, body_bg);

    fb.fill_rect_shadow(wx + 10, wy + 9, 8, 8, fb.encode(0xff, 0x5f, 0x56));
    fb.fill_rect_shadow(wx + 24, wy + 9, 8, 8, fb.encode(0xff, 0xbd, 0x2e));
    fb.fill_rect_shadow(wx + 38, wy + 9, 8, 8, fb.encode(0x27, 0xc9, 0x3f));

    let effective_title = if title.is_empty() { b"Wayland App" } else { title };
    draw_text_alpha_7px(fb, wx + 54, wy + 5, wx + w - 46, effective_title, (0xe6, 0xee, 0xf8));

    let mut badge = [b' '; 6];
    badge[..3].copy_from_slice(b"wl#");
    write_u64_ascii(&mut badge[3..], surface_id as u64);
    let badge_len = if surface_id >= 10 { 5 } else { 4 };
    draw_text_micro(fb, wx + w - 40, wy + 9, &badge[..badge_len], (0x00, 0xd4, 0xb2));
}

pub fn render_kickoff_menu(fb: Framebuffer, selected_category: u8, selected_app: u8) {
    let (mx, my, mw, mh, sidebar_w, cat_pitch, row_h) = kickoff_layout(fb);

    let panel_bg = fb.encode(0x20, 0x24, 0x2f);
    let sidebar_bg = fb.encode(0x1b, 0x1e, 0x28);
    let header_bg = fb.encode(0x22, 0x26, 0x32);
    let footer_bg = fb.encode(0x1a, 0x1d, 0x26);
    let border_col = fb.encode(0x3a, 0x41, 0x54);
    let divider_col = fb.encode(0x2d, 0x33, 0x44);

    blend_rect_shadow(fb, mx + 4, my + 4, mw, mh, 0x05, 0x07, 0x10, 150);

    fb.fill_rect_shadow(mx, my, mw, mh, border_col);
    fb.fill_rect_shadow(mx + 1, my + 1, mw - 2, mh - 2, panel_bg);

    let header_h = 42u32;
    fb.fill_rect_shadow(mx + 1, my + 1, mw - 2, header_h, header_bg);
    fb.fill_rect_shadow(mx + 1, my + header_h, mw - 2, 1, divider_col);

    // Exo-OS native identity avatar ("E") & user/session header
    fill_circle_shadow(fb, (mx + 22) as i32, (my + 22) as i32, 12, (0x00, 0xd4, 0xb2));
    fill_circle_shadow(fb, (mx + 22) as i32, (my + 22) as i32, 10, (0x16, 0x26, 0x34));
    draw_text_micro(fb, mx + 19, my + 18, b"E", (0x54, 0xf2, 0xd8));
    draw_text_alpha(fb, mx + 40, my + 14, mx + 142, b"exo@exo-os", (0xe8, 0xf0, 0xfa));

    // Exo-OS Launcher badge / filter box
    let sx = mx + sidebar_w + 6;
    let sw = mw.saturating_sub(sidebar_w + 54);
    let sy = my + 9;
    let sh = 24u32;
    fb.fill_rect_shadow(sx, sy, sw, sh, fb.encode(0x34, 0x3b, 0x4e));
    fb.fill_rect_shadow(sx + 1, sy + 1, sw - 2, sh - 2, fb.encode(0x13, 0x16, 0x1f));
    draw_circle_ring(fb, (sx + 12) as i32, (sy + 11) as i32, 4, (0x00, 0xd4, 0xb2));
    draw_line_shadow(
        fb,
        (sx + 15) as i32,
        (sy + 14) as i32,
        (sx + 18) as i32,
        (sy + 17) as i32,
        (0x00, 0xd4, 0xb2),
    );
    draw_text_micro_clipped(fb, sx + 24, sy + 9, sx + sw - 4, b"Strata Launcher (FHD)", (0x92, 0xa2, 0xc2));

    draw_filter_icon(fb, mx + mw - 40, my + 15, (0xa0, 0xaa, 0xbe));
    draw_pin_icon(fb, mx + mw - 22, my + 15, (0xa0, 0xaa, 0xbe));

    let body_y = my + header_h + 1;
    let footer_h = 38u32;
    let footer_y = my + mh - footer_h - 1;
    let body_h = footer_y.saturating_sub(body_y);

    // Left Category Sidebar
    fb.fill_rect_shadow(mx + 1, body_y, sidebar_w - 1, body_h, sidebar_bg);
    fb.fill_rect_shadow(mx + sidebar_w, body_y, 1, body_h, divider_col);

    let mut c_idx = 0usize;
    while c_idx < KICKOFF_CATEGORIES.len() {
        let label = KICKOFF_CATEGORIES[c_idx];
        let kind = c_idx as u8;
        let cy = body_y + KICKOFF_CAT_Y_OFFSET + (c_idx as u32) * cat_pitch;
        if cy + cat_pitch - 2 > footer_y {
            break;
        }
        let active = selected_category == kind;
        if active {
            let pill_x = mx + 5;
            let pill_w = sidebar_w - 9;
            fb.fill_rect_shadow(pill_x, cy, pill_w, cat_pitch - 6, fb.encode(0x3d, 0xae, 0xe9));
            fb.fill_rect_shadow(
                pill_x + 1,
                cy + 1,
                pill_w - 2,
                cat_pitch - 8,
                fb.encode(0x28, 0x4b, 0x6e),
            );
        }
        draw_category_icon(
            fb,
            mx + 11,
            cy + (cat_pitch / 2).saturating_sub(8),
            kind,
            if active { (0xff, 0xff, 0xff) } else { (0xb2, 0xbc, 0xce) },
        );
        draw_text_micro_clipped(
            fb,
            mx + 29,
            cy + (cat_pitch / 2).saturating_sub(5),
            mx + sidebar_w - 6,
            label,
            if active { (0xf2, 0xf6, 0xff) } else { (0xc8, 0xd0, 0xde) },
        );
        c_idx += 1;
    }

    // Right Application List
    let list_x = mx + sidebar_w + 8;
    let list_w = mw.saturating_sub(sidebar_w + 16);
    let cat_title = KICKOFF_CATEGORIES[(selected_category as usize).min(KICKOFF_CATEGORIES.len() - 1)];
    draw_text_micro_clipped(fb, list_x + 4, body_y + 6, list_x + list_w, cat_title, (0x7e, 0x88, 0x9c));

    let items = kickoff_items_for_category(selected_category);
    let mut r_idx = 0usize;
    while r_idx < items.len() {
        let item = &items[r_idx];
        let ry = body_y + 20 + (r_idx as u32) * (row_h + 2);
        if ry + row_h > footer_y {
            break;
        }
        render_app_row(
            fb,
            list_x,
            ry,
            list_w,
            row_h,
            item.icon,
            item.title,
            item.subtitle,
            selected_app == r_idx as u8,
        );
        r_idx += 1;
    }

    // Footer Bar
    fb.fill_rect_shadow(mx + 1, footer_y, mw - 2, footer_h, footer_bg);
    fb.fill_rect_shadow(mx + 1, footer_y, mw - 2, 1, divider_col);
    fb.fill_rect_shadow(mx + 1, footer_y, 88, 2, fb.encode(0x3d, 0xae, 0xe9));

    draw_category_icon(fb, mx + 10, footer_y + 13, 1, (0xe6, 0xee, 0xf8));
    draw_text_micro(fb, mx + 26, footer_y + 16, b"Apps", (0xf0, 0xf4, 0xfc));

    draw_category_icon(fb, mx + 92, footer_y + 13, 0, (0x00, 0xd4, 0xb2));
    draw_text_micro(fb, mx + 108, footer_y + 16, b"Terminal", (0xd4, 0xe6, 0xff));

    draw_text_micro(fb, mx + mw - 184, footer_y + 16, b"Calendar", (0xff, 0xd1, 0x66));
    draw_text_micro(fb, mx + mw - 118, footer_y + 16, b"Restart", (0xb4, 0xbe, 0xd0));
    draw_text_micro(fb, mx + mw - 62, footer_y + 16, b"PowerOff", (0xff, 0x7b, 0x72));
}

fn render_app_row(
    fb: Framebuffer,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    icon_kind: u8,
    title: &[u8],
    subtitle: &[u8],
    selected: bool,
) {
    if selected {
        fb.fill_rect_shadow(x, y, w, h, fb.encode(0x3d, 0xae, 0xe9));
        fb.fill_rect_shadow(x + 1, y + 1, w - 2, h - 2, fb.encode(0x21, 0x38, 0x52));
    }

    draw_app_icon_22(fb, x + 6, y + (h.saturating_sub(22)) / 2, icon_kind);

    let text_x = x + 34;
    let max_x = x + w - 4;
    let title_y = y + (h / 2).saturating_sub(17);
    let sub_y = y + (h / 2).saturating_add(4);
    draw_text_alpha_7px(fb, text_x, title_y, max_x, title, (0xed, 0xf2, 0xfa));
    draw_text_micro_clipped(fb, text_x, sub_y, max_x, subtitle, (0x8c, 0x96, 0xaa));
}

pub fn render_bottom_panel_with_wayland(fb: Framebuffer, wayland: &WaylandState) {
    let dock_y = fb.height.saturating_sub(DOCK_BAR_H);
    let panel_bg = fb.encode(0x18, 0x1b, 0x24);
    let top_border = fb.encode(0x2e, 0x34, 0x46);

    fb.fill_rect_shadow(0, dock_y, fb.width, 1, top_border);
    fb.fill_rect_shadow(0, dock_y + 1, fb.width, DOCK_BAR_H - 1, panel_bg);

    // Launcher button emblem (`x = 4..40`)
    if wayland.kickoff_open {
        fb.fill_rect_shadow(4, dock_y + 2, 34, DOCK_BAR_H - 4, fb.encode(0x26, 0x2c, 0x3c));
        fb.fill_rect_shadow(4, dock_y + 1, 34, 2, fb.encode(0x3d, 0xae, 0xe9));
    }
    draw_launcher_emblem(fb, 11, dock_y + 8);

    // Quick-launch Dock Icons (`44..172`)
    if !wayland.surfaces[0].minimized {
        let ind = if wayland.surfaces[0].focused {
            fb.encode(0x3d, 0xae, 0xe9)
        } else {
            fb.encode(0x4c, 0x62, 0x80)
        };
        fb.fill_rect_shadow(44, dock_y + 1, 26, 2, ind);
        fb.fill_rect_shadow(44, dock_y + 3, 26, DOCK_BAR_H - 5, fb.encode(0x23, 0x29, 0x38));
    }
    draw_app_icon_22(fb, 46, dock_y + 7, 0); // ExoTerm
    draw_app_icon_22(fb, 78, dock_y + 7, 3); // ExoFS Storage
    draw_app_icon_22(fb, 110, dock_y + 7, 5); // Wayland Compositor
    draw_app_icon_22(fb, 142, dock_y + 7, 6); // ExoDoctor & Shield

    let clock_x = fb.width.saturating_sub(96);
    let tray_x = clock_x.saturating_sub(154);

    // Dynamic Window Task Pills (ALL active committed toplevel surfaces, including ExoTerm `idx = 0`)
    let mut pill_x = 180u32;
    let pill_w = 136u32;
    let mut idx = 0usize;
    while idx < MAX_SHM_SURFACES && pill_x + pill_w < tray_x {
        let s = &wayland.surfaces[idx];
        if s.active && s.committed {
            let pill_bg = if s.focused && !s.minimized {
                fb.encode(0x24, 0x3b, 0x58)
            } else if s.minimized {
                fb.encode(0x1b, 0x1e, 0x28)
            } else {
                fb.encode(0x20, 0x25, 0x33)
            };
            let pill_top = if s.focused && !s.minimized {
                fb.encode(0x00, 0xd4, 0xb2)
            } else if s.minimized {
                fb.encode(0xff, 0xbd, 0x2e)
            } else {
                fb.encode(0x52, 0x60, 0x78)
            };
            fb.fill_rect_shadow(pill_x, dock_y + 1, pill_w, 2, pill_top);
            fb.fill_rect_shadow(pill_x, dock_y + 3, pill_w, DOCK_BAR_H - 6, pill_bg);
            draw_text_micro_clipped(
                fb,
                pill_x + 8,
                dock_y + 14,
                pill_x + pill_w - 6,
                s.title_str(),
                if s.minimized { (0x92, 0x9e, 0xb4) } else { (0xe6, 0xee, 0xfa) },
            );
            pill_x = pill_x.saturating_add(pill_w + 6);
        }
        idx += 1;
    }

    // Live Wayland badge (`WL:N`) + Tray + Live UTC Clock & Date
    let active_wl = wayland.active_surface_count();
    let mut wl_badge = [b'W', b'L', b':', b'1', b' '];
    wl_badge[3] = b'0' + (active_wl.min(9) as u8);
    draw_text_micro(fb, tray_x.saturating_sub(38), dock_y + 14, &wl_badge[..4], (0x00, 0xd4, 0xb2));

    let tray_y = dock_y + 11;
    draw_tray_icons(fb, tray_x, tray_y, (0xc4, 0xcd, 0xde));

    let dt = rtc_now_utc();
    let mut time_buf = [b' '; 9];
    let t_len = format_clock_hm(&dt, &mut time_buf);
    let mut date_buf = [b' '; 11];
    let d_len = format_date_line(&dt, &mut date_buf);

    draw_text_micro(fb, clock_x + 12, dock_y + 7, &time_buf[..t_len], (0xe8, 0xee, 0xf8));
    draw_text_micro(fb, clock_x + 8, dock_y + 20, &date_buf[..d_len], (0xb2, 0xbc, 0xd0));
}

#[inline]
pub fn write_shadow_px(row_addr: usize, x: u32, bpp: usize, pixel: u32) {
    unsafe {
        if bpp == 4 {
            core::ptr::write((row_addr + (x as usize) * 4) as *mut u32, pixel);
        } else if bpp == 3 {
            let bytes = pixel.to_le_bytes();
            let addr = row_addr + (x as usize) * 3;
            core::ptr::write(addr as *mut u8, bytes[0]);
            core::ptr::write((addr + 1) as *mut u8, bytes[1]);
            core::ptr::write((addr + 2) as *mut u8, bytes[2]);
        }
    }
}

fn blend_rect_shadow(
    fb: Framebuffer,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    sr: u32,
    sg: u32,
    sb: u32,
    alpha: u32,
) {
    if !fb.is_present() || w == 0 || h == 0 {
        return;
    }
    let x_end = x.saturating_add(w).min(fb.width);
    let y_end = y.saturating_add(h).min(fb.height);
    let inv_a = 255 - alpha.min(255);
    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();
    let is_bgr = fb.format == 1;

    let mut ry = y;
    while ry < y_end {
        let row_addr = base.saturating_add((ry as usize).saturating_mul(stride));
        let mut rx = x;
        while rx < x_end {
            let dst = fb.read_pixel(rx, ry);
            let (dr, dg, db) = if is_bgr {
                (dst & 0xff, (dst >> 8) & 0xff, (dst >> 16) & 0xff)
            } else {
                ((dst >> 16) & 0xff, (dst >> 8) & 0xff, dst & 0xff)
            };
            let nr = (sr * alpha + dr * inv_a) / 255;
            let ng = (sg * alpha + dg * inv_a) / 255;
            let nb = (sb * alpha + db * inv_a) / 255;
            let col = if is_bgr {
                (nb << 16) | (ng << 8) | nr
            } else {
                (nr << 16) | (ng << 8) | nb
            };
            write_shadow_px(row_addr, rx, bpp, col);
            rx += 1;
        }
        ry += 1;
    }
}

fn fill_circle_shadow(fb: Framebuffer, cx: i32, cy: i32, r: i32, rgb: (u8, u8, u8)) {
    let col = fb.encode(rgb.0, rgb.1, rgb.2);
    let r2 = r * r;
    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();
    let mut dy = -r;
    while dy <= r {
        let py = cy + dy;
        if py >= 0 && (py as u32) < fb.height {
            let row_addr = base.saturating_add((py as usize).saturating_mul(stride));
            let mut dx = -r;
            while dx <= r {
                if dx * dx + dy * dy <= r2 {
                    let px = cx + dx;
                    if px >= 0 && (px as u32) < fb.width {
                        write_shadow_px(row_addr, px as u32, bpp, col);
                    }
                }
                dx += 1;
            }
        }
        dy += 1;
    }
}

fn draw_circle_ring(fb: Framebuffer, cx: i32, cy: i32, r: i32, rgb: (u8, u8, u8)) {
    let col = fb.encode(rgb.0, rgb.1, rgb.2);
    let r_out = r * r + r;
    let r_in = (r - 1).max(0) * (r - 1).max(0);
    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();
    let mut dy = -r;
    while dy <= r {
        let py = cy + dy;
        if py >= 0 && (py as u32) < fb.height {
            let row_addr = base.saturating_add((py as usize).saturating_mul(stride));
            let mut dx = -r;
            while dx <= r {
                let d2 = dx * dx + dy * dy;
                if d2 <= r_out && d2 >= r_in {
                    let px = cx + dx;
                    if px >= 0 && (px as u32) < fb.width {
                        write_shadow_px(row_addr, px as u32, bpp, col);
                    }
                }
                dx += 1;
            }
        }
        dy += 1;
    }
}

fn draw_line_shadow(
    fb: Framebuffer,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    rgb: (u8, u8, u8),
) {
    let col = fb.encode(rgb.0, rgb.1, rgb.2);
    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x0 >= 0 && y0 >= 0 && (x0 as u32) < fb.width && (y0 as u32) < fb.height {
            let row_addr = base.saturating_add((y0 as usize).saturating_mul(stride));
            write_shadow_px(row_addr, x0 as u32, bpp, col);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = err * 2;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn draw_launcher_emblem(fb: Framebuffer, x: u32, y: u32) {
    let teal = fb.encode(0x00, 0xd4, 0xb2);
    let cyan = fb.encode(0x3d, 0xae, 0xe9);
    fb.fill_rect_shadow(x + 4, y + 1, 12, 3, teal);
    fb.fill_rect_shadow(x + 1, y + 4, 4, 12, cyan);
    fb.fill_rect_shadow(x + 4, y + 16, 12, 3, teal);
    fb.fill_rect_shadow(x + 14, y + 4, 4, 3, cyan);
    fb.fill_rect_shadow(x + 14, y + 13, 4, 3, teal);
}

fn draw_filter_icon(fb: Framebuffer, x: u32, y: u32, rgb: (u8, u8, u8)) {
    let col = fb.encode(rgb.0, rgb.1, rgb.2);
    fb.fill_rect_shadow(x, y + 2, 12, 1, col);
    fb.fill_rect_shadow(x + 3, y + 1, 2, 3, col);
    fb.fill_rect_shadow(x, y + 6, 12, 1, col);
    fb.fill_rect_shadow(x + 7, y + 5, 2, 3, col);
    fb.fill_rect_shadow(x, y + 10, 12, 1, col);
    fb.fill_rect_shadow(x + 4, y + 9, 2, 3, col);
}

fn draw_pin_icon(fb: Framebuffer, x: u32, y: u32, rgb: (u8, u8, u8)) {
    let col = fb.encode(rgb.0, rgb.1, rgb.2);
    fb.fill_rect_shadow(x + 3, y + 1, 6, 2, col);
    fb.fill_rect_shadow(x + 4, y + 3, 4, 4, col);
    fb.fill_rect_shadow(x + 2, y + 7, 8, 2, col);
    fb.fill_rect_shadow(x + 5, y + 9, 2, 4, col);
}

fn draw_category_icon(fb: Framebuffer, x: u32, y: u32, kind: u8, rgb: (u8, u8, u8)) {
    let col = fb.encode(rgb.0, rgb.1, rgb.2);
    match kind {
        0 => {
            fb.fill_rect_shadow(x + 5, y + 1, 2, 10, col);
            fb.fill_rect_shadow(x + 1, y + 5, 10, 2, col);
            fb.fill_rect_shadow(x + 3, y + 3, 6, 6, col);
        }
        1 => {
            fb.fill_rect_shadow(x + 1, y + 1, 4, 4, col);
            fb.fill_rect_shadow(x + 7, y + 1, 4, 4, col);
            fb.fill_rect_shadow(x + 1, y + 7, 4, 4, col);
            fb.fill_rect_shadow(x + 7, y + 7, 4, 4, col);
        }
        2 => {
            draw_circle_ring(fb, (x + 6) as i32, (y + 6) as i32, 4, rgb);
            fb.fill_rect_shadow(x + 5, y + 5, 2, 2, col);
        }
        3 => {
            fb.fill_rect_shadow(x + 1, y + 2, 10, 8, col);
            fb.fill_rect_shadow(x + 2, y + 3, 8, 6, fb.encode(0x1b, 0x1e, 0x28));
            fb.fill_rect_shadow(x + 4, y + 4, 4, 4, col);
        }
        4 => {
            fb.fill_rect_shadow(x + 2, y + 2, 2, 8, col);
            fb.fill_rect_shadow(x + 8, y + 2, 2, 8, col);
            fb.fill_rect_shadow(x + 5, y + 4, 2, 4, col);
        }
        5 => {
            draw_circle_ring(fb, (x + 6) as i32, (y + 6) as i32, 5, rgb);
            fb.fill_rect_shadow(x + 2, y + 6, 8, 1, col);
            fb.fill_rect_shadow(x + 6, y + 2, 1, 8, col);
        }
        6 => {
            fb.fill_rect_shadow(x + 1, y + 2, 10, 8, col);
            fb.fill_rect_shadow(x + 2, y + 4, 8, 2, fb.encode(0x1b, 0x1e, 0x28));
        }
        _ => {
            draw_filter_icon(fb, x, y, rgb);
        }
    }
}

fn draw_app_icon_22(fb: Framebuffer, x: u32, y: u32, kind: u8) {
    match kind {
        0 => {
            fb.fill_rect_shadow(x, y, 22, 22, fb.encode(0x2a, 0x33, 0x45));
            fb.fill_rect_shadow(x + 2, y + 2, 18, 18, fb.encode(0x11, 0x16, 0x22));
            draw_line_shadow(
                fb,
                (x + 5) as i32,
                (y + 6) as i32,
                (x + 11) as i32,
                (y + 11) as i32,
                (0x00, 0xd4, 0xb2),
            );
            draw_line_shadow(
                fb,
                (x + 11) as i32,
                (y + 11) as i32,
                (x + 5) as i32,
                (y + 16) as i32,
                (0x00, 0xd4, 0xb2),
            );
            fb.fill_rect_shadow(x + 12, y + 14, 6, 2, fb.encode(0x45, 0x9c, 0xff));
        }
        1 => {
            fb.fill_rect_shadow(x + 2, y + 3, 18, 13, fb.encode(0xc8, 0xd0, 0xde));
            fb.fill_rect_shadow(x + 4, y + 5, 14, 9, fb.encode(0x21, 0x26, 0x32));
            fb.fill_rect_shadow(x + 9, y + 16, 4, 2, fb.encode(0x9a, 0xa2, 0xb2));
            fb.fill_rect_shadow(x + 6, y + 18, 10, 2, fb.encode(0x9a, 0xa2, 0xb2));
        }
        2 => {
            fill_circle_shadow(fb, (x + 11) as i32, (y + 11) as i32, 9, (0x00, 0xb8, 0xa8));
            fill_circle_shadow(fb, (x + 11) as i32, (y + 11) as i32, 6, (0x14, 0x2c, 0x38));
            draw_text_micro(fb, x + 9, y + 8, b"E", (0x54, 0xf2, 0xd8));
        }
        3 => {
            fb.fill_rect_shadow(x + 2, y + 4, 8, 3, fb.encode(0x2a, 0x7e, 0xc2));
            fb.fill_rect_shadow(x + 2, y + 6, 18, 12, fb.encode(0x3d, 0xae, 0xe9));
            fb.fill_rect_shadow(x + 3, y + 8, 16, 3, fb.encode(0x73, 0xc8, 0xf5));
        }
        4 => {
            fb.fill_rect_shadow(x + 2, y + 3, 18, 16, fb.encode(0x2a, 0x36, 0x4c));
            fb.fill_rect_shadow(x + 2, y + 3, 18, 4, fb.encode(0xff, 0xb7, 0x03));
            fb.fill_rect_shadow(x + 5, y + 10, 3, 3, fb.encode(0xe6, 0xee, 0xf8));
            fb.fill_rect_shadow(x + 10, y + 10, 3, 3, fb.encode(0x60, 0xff, 0x90));
            fb.fill_rect_shadow(x + 15, y + 10, 3, 3, fb.encode(0xe6, 0xee, 0xf8));
        }
        5 => {
            fill_circle_shadow(fb, (x + 11) as i32, (y + 11) as i32, 9, (0x3a, 0x86, 0xff));
            fb.fill_rect_shadow(x + 5, y + 6, 10, 5, fb.encode(0x00, 0xd4, 0xb2));
            fb.fill_rect_shadow(x + 8, y + 11, 8, 5, fb.encode(0xff, 0xb7, 0x03));
        }
        _ => {
            fill_circle_shadow(fb, (x + 11) as i32, (y + 11) as i32, 9, (0x2e, 0xcc, 0x71));
            fb.fill_rect_shadow(x + 9, y + 6, 4, 10, fb.encode(0x0c, 0x28, 0x18));
            fb.fill_rect_shadow(x + 6, y + 9, 10, 4, fb.encode(0x0c, 0x28, 0x18));
        }
    }
}

fn draw_tray_icons(fb: Framebuffer, x: u32, y: u32, rgb: (u8, u8, u8)) {
    let col = fb.encode(rgb.0, rgb.1, rgb.2);
    fb.fill_rect_shadow(x, y + 1, 9, 11, col);
    fb.fill_rect_shadow(x + 2, y + 3, 5, 7, fb.encode(0x18, 0x1b, 0x24));
    draw_circle_ring(fb, (x + 24) as i32, (y + 6) as i32, 4, rgb);
    fb.fill_rect_shadow(x + 38, y + 4, 3, 4, col);
    fb.fill_rect_shadow(x + 41, y + 2, 3, 8, col);
    fb.fill_rect_shadow(x + 54, y + 2, 8, 8, fb.encode(0x60, 0xff, 0x90));
    fb.fill_rect_shadow(x + 56, y + 4, 4, 4, fb.encode(0x18, 0x1b, 0x24));
    fb.fill_rect_shadow(x + 70, y + 8, 2, 4, col);
    fb.fill_rect_shadow(x + 73, y + 5, 2, 7, col);
    fb.fill_rect_shadow(x + 76, y + 2, 2, 10, col);
    draw_text_micro(fb, x + 88, y + 3, b"^", rgb);
}

fn write_u64_ascii(out: &mut [u8], mut val: u64) {
    if out.is_empty() {
        return;
    }
    let mut tmp = [b'0'; 20];
    let mut n = 0usize;
    if val == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while val > 0 && n < tmp.len() {
            tmp[n] = b'0' + (val % 10) as u8;
            val /= 10;
            n += 1;
        }
    }
    let count = n.min(out.len());
    let mut i = 0usize;
    while i < count {
        out[i] = tmp[n - 1 - i];
        i += 1;
    }
}

pub fn draw_text_alpha(
    fb: Framebuffer,
    px: u32,
    py: u32,
    max_x: u32,
    text: &[u8],
    fg_rgb: (u8, u8, u8),
) {
    draw_text_step(fb, px, py, max_x, text, fg_rgb, FONT_GLYPH_WIDTH as u32);
}

pub fn draw_text_alpha_7px(
    fb: Framebuffer,
    px: u32,
    py: u32,
    max_x: u32,
    text: &[u8],
    fg_rgb: (u8, u8, u8),
) {
    draw_text_step(fb, px, py, max_x, text, fg_rgb, 7);
}

fn draw_text_step(
    fb: Framebuffer,
    px: u32,
    py: u32,
    max_x: u32,
    text: &[u8],
    fg_rgb: (u8, u8, u8),
    advance: u32,
) {
    let fg = fb.encode(fg_rgb.0, fg_rgb.1, fg_rgb.2);
    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();

    let mut cx = px;
    let limit_x = max_x.min(fb.width);
    for &byte in text {
        if cx.saturating_add(7) >= limit_x {
            break;
        }
        let glyph = glyph_for(byte);
        let mut gy = 0usize;
        while gy < FONT_GLYPH_HEIGHT {
            let row_y = py + (gy as u32);
            if row_y < fb.height {
                let bits = glyph[gy];
                if bits != 0 {
                    let row_addr = base.saturating_add((row_y as usize).saturating_mul(stride));
                    let mut gx = 0usize;
                    while gx < FONT_GLYPH_WIDTH {
                        if (bits & (0x80 >> gx)) != 0 {
                            write_shadow_px(row_addr, cx + (gx as u32), bpp, fg);
                        }
                        gx += 1;
                    }
                }
            }
            gy += 1;
        }
        cx += advance;
    }
}

pub fn draw_text_micro(fb: Framebuffer, px: u32, py: u32, text: &[u8], fg_rgb: (u8, u8, u8)) {
    draw_text_micro_clipped(fb, px, py, fb.width, text, fg_rgb);
}

pub fn draw_text_micro_clipped(
    fb: Framebuffer,
    px: u32,
    py: u32,
    max_x: u32,
    text: &[u8],
    fg_rgb: (u8, u8, u8),
) {
    let fg = fb.encode(fg_rgb.0, fg_rgb.1, fg_rgb.2);
    let bpp = fb.bytes_per_pixel() as usize;
    let stride = (fb.stride_pixels as usize).saturating_mul(bpp);
    let base = fb.draw_target_base();
    let limit_x = max_x.min(fb.width);

    let mut cx = px;
    for &byte in text {
        if cx.saturating_add(MICRO_W) >= limit_x {
            break;
        }
        let glyph = glyph_for(byte);
        let mut my = 0u32;
        while my < MICRO_H {
            let row_y = py + my;
            if row_y < fb.height {
                let g_r0 = (2 + (my as usize * 12) / (MICRO_H as usize)).min(15);
                let g_r1 = (g_r0 + 1).min(15);
                let bits = glyph[g_r0] | glyph[g_r1];
                if bits != 0 {
                    let row_addr = base.saturating_add((row_y as usize).saturating_mul(stride));
                    let mut mx = 0usize;
                    while mx < MICRO_W as usize {
                        let mask: u8 = match mx {
                            0 => 0xC0,
                            1 => 0x20,
                            2 => 0x10,
                            3 => 0x08,
                            _ => 0x07,
                        };
                        if (bits & mask) != 0 {
                            write_shadow_px(row_addr, cx + (mx as u32), bpp, fg);
                        }
                        mx += 1;
                    }
                }
            }
            my += 1;
        }
        cx += MICRO_ADV;
    }
}

#[inline]
pub fn blend_argb8888(dst: u32, src: u32) -> u32 {
    let a = (src >> 24) & 0xFF;
    if a == 0 {
        return dst;
    }
    if a == 255 {
        return src & 0x00FF_FFFF;
    }
    let inv_a = 255 - a;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;

    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;

    let r = (sr * a + dr * inv_a) / 255;
    let g = (sg * a + dg * inv_a) / 255;
    let b = (sb * a + db * inv_a) / 255;

    (r << 16) | (g << 8) | b
}

#[inline]
pub fn blend_rgba_to_fb(dst_px: u32, r: u32, g: u32, b: u32, a: u32, is_bgr: bool) -> u32 {
    if a == 0 {
        return dst_px;
    }
    let (nr, ng, nb) = if a >= 255 {
        (r, g, b)
    } else {
        let inv_a = 255 - a;
        let (dr, dg, db) = if is_bgr {
            (dst_px & 0xff, (dst_px >> 8) & 0xff, (dst_px >> 16) & 0xff)
        } else {
            ((dst_px >> 16) & 0xff, (dst_px >> 8) & 0xff, dst_px & 0xff)
        };
        (
            (r * a + dr * inv_a) / 255,
            (g * a + dg * inv_a) / 255,
            (b * a + db * inv_a) / 255,
        )
    };
    if is_bgr {
        (nb << 16) | (ng << 8) | nr
    } else {
        (nr << 16) | (ng << 8) | nb
    }
}
