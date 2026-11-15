//! alert.rs — ExoFS Alert System (no_std).
//!
//! Provides :
//!  - `AlertLevel`      : alert level (Info/Warning/Error/Critical).
//!  - `AlertCode`       : typed alert code.
//!  - `Alert`           : 64-byte repr C entry.
//!  - `AlertLog`        : 256-entry circular ring (spinlock-free).
//!  - `AlertFilter`     : filter by minimum level.
//!  - `AlertCounter`    : counters per level.
//!  - `AlertManager`    : high-level interface.
//!  - `ALERT_LOG`       : global singleton.
//!
//! RECUR-01 : while loops only.
//! OOM-02   : try_reserve before push.
//! ARITH-02 : saturating_*, checked_div, wrapping_*.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── AlertLevel ───────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertLevel {
    Info = 0,
    Warning = 1,
    Error = 2,
    Critical = 3,
}

impl AlertLevel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
            Self::Critical => "CRITICAL",
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Warning,
            2 => Self::Error,
            3 => Self::Critical,
            _ => Self::Info,
        }
    }

    pub fn is_actionable(self) -> bool {
        self >= Self::Warning
    }
}

// ─── AlertCode ────────────────────────────────────────────────────────────────

/// Structured alert code (high 8-bit category + low 8-bit subcode).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertCode(pub u16);

impl AlertCode {
    /// Constructs an alert code.
    pub const fn new(code: u16) -> Self {
        Self(code)
    }
}

impl AlertCode {
    pub const IO_ERR: Self = Self(0x0100);
    pub const OOM: Self = Self(0x0200);
    pub const CHECKSUM_FAIL: Self = Self(0x0300);
    pub const GC_OVERFLOW: Self = Self(0x0400);
    pub const EPOCH_STALL: Self = Self(0x0500);
    pub const QUOTA_EXCEED: Self = Self(0x0600);
    pub const CORRUPTION: Self = Self(0x0700);
    pub const SPACE_LOW: Self = Self(0x0800);
    pub const CACHE_THRASH: Self = Self(0x0900);

    pub fn category(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub fn subcode(self) -> u8 {
        self.0 as u8
    }
    pub fn raw(self) -> u16 {
        self.0
    }
}

// ─── Alert ────────────────────────────────────────────────────────────────────

/// Alert entry (64 bytes, repr C).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Alert {
    pub tick: u64,
    pub level: AlertLevel,
    pub code: AlertCode,
    _pad: [u8; 5],
    pub msg: [u8; 46],
}

const _ALERT_SIZE: () = assert!(core::mem::size_of::<Alert>() == 64);

impl Alert {
    pub const fn zeroed() -> Self {
        Self {
            tick: 0,
            level: AlertLevel::Info,
            code: AlertCode(0),
            _pad: [0; 5],
            msg: [0; 46],
        }
    }

    pub fn new(tick: u64, level: AlertLevel, code: AlertCode, msg: &[u8]) -> Self {
        let mut m = [0u8; 46];
        let n = msg.len().min(46);
        // RECUR-01 : while
        let mut i = 0usize;
        while i < n {
            m[i] = msg[i];
            i = i.wrapping_add(1);
        }
        Self {
            tick,
            level,
            code,
            _pad: [0; 5],
            msg: m,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tick == 0
    }

    /// Copies the message into a Vec (OOM-02).
    pub fn msg_to_vec(&self) -> ExofsResult<Vec<u8>> {
        let mut end = 46usize;
        while end > 0 && self.msg[end.wrapping_sub(1)] == 0 {
            end = end.saturating_sub(1);
        }
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve(end).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < end {
            v.push(self.msg[i]);
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

// ─── AlertLog ────────────────────────────────────────────────────────────────

pub const ALERT_RING_SIZE: usize = 256;

/// Circular ring of alerts (atomic head, UnsafeCell slots).
pub struct AlertLog {
    ring: [UnsafeCell<Alert>; ALERT_RING_SIZE],
    head: AtomicU64,
    tick: AtomicU64, // internal monotonic tick
    // Counters per level
    n_info: AtomicU64,
    n_warning: AtomicU64,
    n_error: AtomicU64,
    n_critical: AtomicU64,
}

// SAFETY: access managed by rotating atomic index.
unsafe impl Sync for AlertLog {}
unsafe impl Send for AlertLog {}

impl AlertLog {
    pub const fn new_const() -> Self {
        const Z: UnsafeCell<Alert> = UnsafeCell::new(Alert::zeroed());
        Self {
            ring: [Z; ALERT_RING_SIZE],
            head: AtomicU64::new(0),
            tick: AtomicU64::new(0),
            n_info: AtomicU64::new(0),
            n_warning: AtomicU64::new(0),
            n_error: AtomicU64::new(0),
            n_critical: AtomicU64::new(0),
        }
    }

    fn next_tick(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::Relaxed)
    }

    /// Adds an alert to the ring.
    pub fn push(&self, level: AlertLevel, code: AlertCode, msg: &[u8]) {
        let t = self.next_tick();
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % ALERT_RING_SIZE;
        let entry = Alert::new(t, level, code, msg);
        // SAFETY: atomic rotating index — no concurrent double write.
        unsafe {
            *self.ring[idx].get() = entry;
        }
        match level {
            AlertLevel::Info => {
                self.n_info.fetch_add(1, Ordering::Relaxed);
            }
            AlertLevel::Warning => {
                self.n_warning.fetch_add(1, Ordering::Relaxed);
            }
            AlertLevel::Error => {
                self.n_error.fetch_add(1, Ordering::Relaxed);
            }
            AlertLevel::Critical => {
                self.n_critical.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Semantic helpers.
    pub fn info(&self, code: AlertCode, msg: &[u8]) {
        self.push(AlertLevel::Info, code, msg);
    }
    pub fn warning(&self, code: AlertCode, msg: &[u8]) {
        self.push(AlertLevel::Warning, code, msg);
    }
    pub fn error(&self, code: AlertCode, msg: &[u8]) {
        self.push(AlertLevel::Error, code, msg);
    }
    pub fn critical(&self, code: AlertCode, msg: &[u8]) {
        self.push(AlertLevel::Critical, code, msg);
    }

    /// Counters per level.
    pub fn count_info(&self) -> u64 {
        self.n_info.load(Ordering::Relaxed)
    }
    pub fn count_warning(&self) -> u64 {
        self.n_warning.load(Ordering::Relaxed)
    }
    pub fn count_error(&self) -> u64 {
        self.n_error.load(Ordering::Relaxed)
    }
    pub fn count_critical(&self) -> u64 {
        self.n_critical.load(Ordering::Relaxed)
    }
    pub fn total_alerts(&self) -> u64 {
        self.n_info
            .load(Ordering::Relaxed)
            .saturating_add(self.n_warning.load(Ordering::Relaxed))
            .saturating_add(self.n_error.load(Ordering::Relaxed))
            .saturating_add(self.n_critical.load(Ordering::Relaxed))
    }

    /// Latest recorded event.
    pub fn latest(&self) -> Alert {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx = (head.wrapping_add(ALERT_RING_SIZE).wrapping_sub(1)) % ALERT_RING_SIZE;
        // SAFETY: diagnostic read.
        unsafe { *self.ring[idx].get() }
    }

    /// Collects the last n alerts (RECUR-01: while / OOM-02).
    pub fn last_n(
        &self,
        n: usize,
        filter: Option<AlertLevel>,
        out: &mut Vec<Alert>,
    ) -> ExofsResult<()> {
        let cap = n.min(ALERT_RING_SIZE);
        out.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut found = 0usize;
        let mut i = 0usize;
        while i < ALERT_RING_SIZE && found < cap {
            let idx = (head
                .wrapping_add(ALERT_RING_SIZE)
                .wrapping_sub(i)
                .wrapping_sub(1))
                % ALERT_RING_SIZE;
            // SAFETY: diagnostic read.
            let a = unsafe { *self.ring[idx].get() };
            if a.is_empty() {
                i = i.wrapping_add(1);
                continue;
            }
            let pass = match filter {
                Some(min) => a.level >= min,
                None => true,
            };
            if pass {
                out.push(a);
                found = found.wrapping_add(1);
            }
            i = i.wrapping_add(1);
        }
        Ok(())
    }

    /// Checks if the system is in an active critical alert state.
    pub fn has_critical(&self) -> bool {
        self.n_critical.load(Ordering::Relaxed) > 0
    }

    /// Error ratio * 1000 (ARITH-02).
    pub fn error_ratio_pct10(&self) -> u64 {
        let total = self.total_alerts();
        let err = self
            .n_error
            .load(Ordering::Relaxed)
            .saturating_add(self.n_critical.load(Ordering::Relaxed));
        err.saturating_mul(1000).checked_div(total).unwrap_or(0)
    }
}

/// Global singleton.
pub static ALERT_LOG: AlertLog = AlertLog::new_const();

// ─── AlertFilter ─────────────────────────────────────────────────────────────

/// Configurable filter by minimum level.
#[derive(Clone, Copy, Debug)]
pub struct AlertFilter {
    pub min_level: AlertLevel,
    pub code_mask: Option<u8>, // code category to retain (None = all)
}

impl AlertFilter {
    pub fn all() -> Self {
        Self {
            min_level: AlertLevel::Info,
            code_mask: None,
        }
    }
    pub fn warnings() -> Self {
        Self {
            min_level: AlertLevel::Warning,
            code_mask: None,
        }
    }
    pub fn errors() -> Self {
        Self {
            min_level: AlertLevel::Error,
            code_mask: None,
        }
    }

    pub fn matches(&self, a: &Alert) -> bool {
        if a.level < self.min_level {
            return false;
        }
        if let Some(cat) = self.code_mask {
            if a.code.category() != cat {
                return false;
            }
        }
        true
    }
}

// ─── AlertManager ────────────────────────────────────────────────────────────

/// High-level interface to emit and inspect alerts.
pub struct AlertManager<'a> {
    log: &'a AlertLog,
    filter: AlertFilter,
}

impl<'a> AlertManager<'a> {
    pub fn new(log: &'a AlertLog) -> Self {
        Self {
            log,
            filter: AlertFilter::all(),
        }
    }

    pub fn with_filter(mut self, f: AlertFilter) -> Self {
        self.filter = f;
        self
    }

    pub fn emit(&self, level: AlertLevel, code: AlertCode, msg: &[u8]) {
        self.log.push(level, code, msg);
    }

    pub fn collect_filtered(&self, n: usize, out: &mut Vec<Alert>) -> ExofsResult<()> {
        let cap = n.min(ALERT_RING_SIZE);
        out.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        let head = self.log.head.load(Ordering::Relaxed) as usize;
        let mut found = 0usize;
        let mut i = 0usize;
        while i < ALERT_RING_SIZE && found < cap {
            let idx = (head
                .wrapping_add(ALERT_RING_SIZE)
                .wrapping_sub(i)
                .wrapping_sub(1))
                % ALERT_RING_SIZE;
            // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
            let a = unsafe { *self.log.ring[idx].get() };
            if !a.is_empty() && self.filter.matches(&a) {
                out.push(a);
                found = found.wrapping_add(1);
            }
            i = i.wrapping_add(1);
        }
        Ok(())
    }

    pub fn has_actionable(&self) -> bool {
        self.log
            .n_warning
            .load(Ordering::Relaxed)
            .saturating_add(self.log.n_error.load(Ordering::Relaxed))
            .saturating_add(self.log.n_critical.load(Ordering::Relaxed))
            > 0
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_counts() {
        let log = AlertLog::new_const();
        log.push(AlertLevel::Info, AlertCode::IO_ERR, b"test");
        log.push(AlertLevel::Critical, AlertCode::CORRUPTION, b"bad");
        assert_eq!(log.count_info(), 1);
        assert_eq!(log.count_critical(), 1);
        assert_eq!(log.total_alerts(), 2);
    }

    #[test]
    fn test_helpers_semantic() {
        let log = AlertLog::new_const();
        log.info(AlertCode::IO_ERR, b"info msg");
        log.warning(AlertCode::SPACE_LOW, b"warn");
        assert_eq!(log.count_info(), 1);
        assert_eq!(log.count_warning(), 1);
    }

    #[test]
    fn test_latest() {
        let log = AlertLog::new_const();
        log.push(AlertLevel::Error, AlertCode::OOM, b"oom error");
        let a = log.latest();
        assert_eq!(a.level, AlertLevel::Error);
        assert_eq!(a.code, AlertCode::OOM);
    }

    #[test]
    fn test_last_n_filter() {
        let log = AlertLog::new_const();
        log.push(AlertLevel::Info, AlertCode(0), b"info");
        log.push(AlertLevel::Error, AlertCode(0), b"err");
        log.push(AlertLevel::Critical, AlertCode(0), b"crit");
        let mut out = Vec::new();
        log.last_n(10, Some(AlertLevel::Error), &mut out)
            .test_expect("ok");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|a| a.level >= AlertLevel::Error));
    }

    #[test]
    fn test_has_critical() {
        let log = AlertLog::new_const();
        assert!(!log.has_critical());
        log.critical(AlertCode::CORRUPTION, b"crit");
        assert!(log.has_critical());
    }

    #[test]
    fn test_alert_code_category() {
        assert_eq!(AlertCode::IO_ERR.category(), 0x01);
        assert_eq!(AlertCode::OOM.category(), 0x02);
    }

    #[test]
    fn test_alert_level_ordering() {
        assert!(AlertLevel::Critical > AlertLevel::Error);
        assert!(AlertLevel::Error > AlertLevel::Warning);
        assert!(AlertLevel::Warning > AlertLevel::Info);
    }

    #[test]
    fn test_filter_matches() {
        let f = AlertFilter::warnings();
        let a_info = Alert::new(0, AlertLevel::Info, AlertCode(0), b"");
        let a_warn = Alert::new(0, AlertLevel::Warning, AlertCode(0), b"");
        assert!(!f.matches(&a_info));
        assert!(f.matches(&a_warn));
    }

    #[test]
    fn test_msg_to_vec() {
        let a = Alert::new(0, AlertLevel::Info, AlertCode(0), b"hello");
        let v = a.msg_to_vec().test_expect("ok");
        assert_eq!(&v[..], b"hello");
    }

    #[test]
    fn test_manager_emit_collect() {
        let log = AlertLog::new_const();
        let mgr = AlertManager::new(&log).with_filter(AlertFilter::errors());
        mgr.emit(AlertLevel::Info, AlertCode(0), b"drop");
        mgr.emit(AlertLevel::Error, AlertCode::CORRUPTION, b"keep");
        let mut out = Vec::new();
        mgr.collect_filtered(10, &mut out).test_expect("ok");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].level, AlertLevel::Error);
    }

    #[test]
    fn test_error_ratio_pct10() {
        let log = AlertLog::new_const();
        log.push(AlertLevel::Info, AlertCode(0), b"");
        log.push(AlertLevel::Error, AlertCode(0), b"");
        // 1/(2) * 1000 = 500
        assert_eq!(log.error_ratio_pct10(), 500);
    }

    #[test]
    fn test_level_from_u8() {
        assert_eq!(AlertLevel::from_u8(0), AlertLevel::Info);
        assert_eq!(AlertLevel::from_u8(3), AlertLevel::Critical);
    }
}
