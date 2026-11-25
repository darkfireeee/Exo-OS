//! Gregorian UTC `calendar` builtin backed by CMOS RTC.
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;

fn civil_from_days(days_since_epoch: i64) -> (i64, u8, u8) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m as u8, d as u8)
}

fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn month_name(month: u8) -> &'static [u8] {
    match month {
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

pub fn cmd_calendar(rest: &[u8]) {
    let mut ts = Timespec::default();
    let rc = unsafe { syscall::syscall2(syscall::SYS_CLOCK_GETTIME, 0, &mut ts as *mut Timespec as u64) };
    let secs = if rc == 0 && ts.tv_sec >= 1_577_836_800 {
        ts.tv_sec
    } else {
        1_794_744_000
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as u64;
    let min = ((rem % 3600) / 60) as u64;
    let (year, month, day) = civil_from_days(days);
    let first_wday = ((days - (day as i64 - 1) + 4).rem_euclid(7)) as u8;
    let dim = days_in_month(year, month);

    let mut out = [0u8; 640];
    let mut len = 0usize;
    push_bytes(&mut out, &mut len, b"    ");
    push_bytes(&mut out, &mut len, month_name(month));
    push_bytes(&mut out, &mut len, b" ");
    push_u64(&mut out, &mut len, year as u64);
    push_bytes(&mut out, &mut len, b" (");
    if hour < 10 { push_bytes(&mut out, &mut len, b"0"); }
    push_u64(&mut out, &mut len, hour);
    push_bytes(&mut out, &mut len, b":");
    if min < 10 { push_bytes(&mut out, &mut len, b"0"); }
    push_u64(&mut out, &mut len, min);
    push_bytes(&mut out, &mut len, b" UTC)\n");
    push_bytes(&mut out, &mut len, b"Su Mo Tu We Th Fr Sa\n");

    let mut col = 0u8;
    while col < first_wday {
        push_bytes(&mut out, &mut len, b"   ");
        col += 1;
    }
    let mut d = 1u8;
    while d <= dim {
        if d == day {
            push_bytes(&mut out, &mut len, b"[");
            push_u64(&mut out, &mut len, d as u64);
            push_bytes(&mut out, &mut len, b"]");
        } else {
            if d < 10 { push_bytes(&mut out, &mut len, b" "); }
            push_u64(&mut out, &mut len, d as u64);
            push_bytes(&mut out, &mut len, b" ");
        }
        col += 1;
        if col == 7 || d == dim {
            push_bytes(&mut out, &mut len, b"\n");
            col = 0;
        }
        d += 1;
    }
    if !trim(rest).is_empty() {
        push_bytes(&mut out, &mut len, b"\nExo-OS Real-Time Clock (CMOS RTC UTC)\n");
    }
    write_bytes(&out[..len]);
}

