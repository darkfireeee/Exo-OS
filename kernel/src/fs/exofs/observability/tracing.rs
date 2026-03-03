// SPDX-License-Identifier: MIT
// ExoFS Observability — Tracing
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── Constants ────────────────────────────────────────────────────────────────

pub const TRACE_RING_SIZE:    usize = 512;
pub const TRACE_MSG_LEN:      usize = 112;
pub const TRACE_COMPONENT_ALL: u32  = u32::MAX;

// ─── TraceLevel ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum TraceLevel {
    Off   = 0,
    Error = 1,
    Warn  = 2,
    Info  = 3,
    Debug = 4,
    Trace = 5,
}

impl TraceLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Error,
            2 => Self::Warn,
            3 => Self::Info,
            4 => Self::Debug,
            5 => Self::Trace,
            _ => Self::Off,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Off   => "OFF",
            Self::Error => "ERROR",
            Self::Warn  => "WARN",
            Self::Info  => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
    pub fn is_active(self) -> bool { self != Self::Off }
    pub fn is_verbose(self) -> bool { self >= Self::Debug }
}

// ─── ComponentId ─────────────────────────────────────────────────────────────

/// Masque binaire identifiant un sous-composant ExoFS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ComponentId(pub u32);

impl ComponentId {
    pub const CORE:      ComponentId = ComponentId(1 << 0);
    pub const IO:        ComponentId = ComponentId(1 << 1);
    pub const MEMORY:    ComponentId = ComponentId(1 << 2);
    pub const GC:        ComponentId = ComponentId(1 << 3);
    pub const INDEX:     ComponentId = ComponentId(1 << 4);
    pub const CACHE:     ComponentId = ComponentId(1 << 5);
    pub const JOURNAL:   ComponentId = ComponentId(1 << 6);
    pub const SECURITY:  ComponentId = ComponentId(1 << 7);
    pub const OBSERVER:  ComponentId = ComponentId(1 << 8);
    pub const ALL:       ComponentId = ComponentId(u32::MAX);

    pub fn name(self) -> &'static str {
        match self.0 {
            x if x == Self::CORE.0     => "CORE",
            x if x == Self::IO.0       => "IO",
            x if x == Self::MEMORY.0   => "MEMORY",
            x if x == Self::GC.0       => "GC",
            x if x == Self::INDEX.0    => "INDEX",
            x if x == Self::CACHE.0    => "CACHE",
            x if x == Self::JOURNAL.0  => "JOURNAL",
            x if x == Self::SECURITY.0 => "SECURITY",
            x if x == Self::OBSERVER.0 => "OBSERVER",
            _                          => "UNKNOWN",
        }
    }
}

// ─── TraceEvent ───────────────────────────────────────────────────────────────

/// Événement de trace de 128 octets (repr C, taille fixe, copiable).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct TraceEvent {
    pub tick:         u64,
    pub component:    u32,
    pub level:        u8,
    pub _pad:         [u8; 3],
    pub msg:          [u8; TRACE_MSG_LEN],
}

impl TraceEvent {
    const _SIZE_CHECK: () = assert!(
        core::mem::size_of::<TraceEvent>() == 128,
        "TraceEvent must be 128 bytes"
    );

    pub const fn zeroed() -> Self {
        Self {
            tick:      0,
            component: 0,
            level:     0,
            _pad:      [0; 3],
            msg:       [0; TRACE_MSG_LEN],
        }
    }

    /// Construit un événement depuis un &str (tronqué à TRACE_MSG_LEN octets).
    pub fn new(tick: u64, comp: ComponentId, level: TraceLevel, text: &str) -> Self {
        let mut evt = Self::zeroed();
        evt.tick      = tick;
        evt.component = comp.0;
        evt.level     = level as u8;
        let bytes = text.as_bytes();
        let len = bytes.len().min(TRACE_MSG_LEN);
        let mut i = 0usize;
        while i < len { evt.msg[i] = bytes[i]; i = i.wrapping_add(1); }
        evt
    }

    pub fn is_empty(&self) -> bool { self.level == 0 && self.tick == 0 }

    pub fn trace_level(&self) -> TraceLevel {
        TraceLevel::from_u8(self.level)
    }

    /// Copie le message dans un Vec (OOM-02).
    pub fn msg_to_vec(&self) -> ExofsResult<Vec<u8>> {
        let len = self.msg.iter().position(|&b| b == 0).unwrap_or(TRACE_MSG_LEN);
        let mut v = Vec::new();
        v.try_reserve(len).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < len { v.push(self.msg[i]); i = i.wrapping_add(1); }
        Ok(v)
    }
}

// ─── TraceFilter ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct TraceFilter {
    pub min_level:      TraceLevel,
    pub component_mask: u32,
}

impl TraceFilter {
    pub const fn all()    -> Self { Self { min_level: TraceLevel::Trace, component_mask: u32::MAX } }
    pub const fn errors() -> Self { Self { min_level: TraceLevel::Error, component_mask: u32::MAX } }
    pub const fn warnings() -> Self { Self { min_level: TraceLevel::Warn, component_mask: u32::MAX } }

    pub fn for_component(comp: ComponentId) -> Self {
        Self { min_level: TraceLevel::Trace, component_mask: comp.0 }
    }

    pub fn matches(&self, evt: &TraceEvent) -> bool {
        let lvl = TraceLevel::from_u8(evt.level);
        lvl >= self.min_level && (evt.component & self.component_mask) != 0
    }
}

// ─── TraceRing ────────────────────────────────────────────────────────────────

pub struct TraceRing {
    events:  UnsafeCell<[TraceEvent; TRACE_RING_SIZE]>,
    head:    AtomicU64,
    count:   AtomicU64,
    dropped: AtomicU64,
    /// Niveau minimum actif (Off = ring désactivé).
    min_level: AtomicU64,
}

unsafe impl Sync for TraceRing {}
unsafe impl Send for TraceRing {}

impl TraceRing {
    pub const fn new_const() -> Self {
        Self {
            events:    UnsafeCell::new([TraceEvent::zeroed(); TRACE_RING_SIZE]),
            head:      AtomicU64::new(0),
            count:     AtomicU64::new(0),
            dropped:   AtomicU64::new(0),
            min_level: AtomicU64::new(TraceLevel::Info as u64),
        }
    }

    pub fn set_min_level(&self, lvl: TraceLevel) {
        self.min_level.store(lvl as u64, Ordering::Relaxed);
    }
    pub fn min_level(&self) -> TraceLevel {
        TraceLevel::from_u8(self.min_level.load(Ordering::Relaxed) as u8)
    }

    pub fn push(&self, evt: TraceEvent) {
        let lvl = TraceLevel::from_u8(evt.level);
        if lvl < self.min_level() { return; }
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % TRACE_RING_SIZE;
        unsafe { (*self.events.get())[idx] = evt; }
        let n = self.count.load(Ordering::Relaxed);
        if n < TRACE_RING_SIZE as u64 {
            self.count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Émission simplifiée par composant/level/message.
    pub fn emit(&self, tick: u64, comp: ComponentId, level: TraceLevel, msg: &str) {
        self.push(TraceEvent::new(tick, comp, level, msg));
    }

    pub fn latest(&self) -> Option<TraceEvent> {
        if self.count.load(Ordering::Relaxed) == 0 { return None; }
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx = (head.wrapping_add(TRACE_RING_SIZE).wrapping_sub(1)) % TRACE_RING_SIZE;
        let evt = unsafe { (*self.events.get())[idx] };
        if evt.is_empty() { None } else { Some(evt) }
    }

    /// Retourne les n derniers événements filtrés (OOM-02, RECUR-01).
    pub fn last_n_filtered(&self, n: usize, filter: &TraceFilter) -> ExofsResult<Vec<TraceEvent>> {
        let count = self.count.load(Ordering::Relaxed) as usize;
        let capacity = n.min(count);
        let mut v = Vec::new();
        v.try_reserve(capacity).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < count && v.len() < n {
            let idx = (head.wrapping_add(TRACE_RING_SIZE).wrapping_sub(i + 1)) % TRACE_RING_SIZE;
            let evt = unsafe { (*self.events.get())[idx] };
            if filter.matches(&evt) {
                v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                v.push(evt);
            }
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    pub fn count(&self)   -> u64 { self.count.load(Ordering::Relaxed) }
    pub fn dropped(&self) -> u64 { self.dropped.load(Ordering::Relaxed) }

    pub fn reset(&self) {
        let mut i = 0usize;
        while i < TRACE_RING_SIZE {
            unsafe { (*self.events.get())[i] = TraceEvent::zeroed(); }
            i = i.wrapping_add(1);
        }
        self.head.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
    }
}

pub static TRACE_RING: TraceRing = TraceRing::new_const();

// ─── TraceSession ─────────────────────────────────────────────────────────────

/// Session de capture de traces pour un composant.
pub struct TraceSession<'a> {
    ring:      &'a TraceRing,
    component: ComponentId,
    tick:      AtomicU64,
}

impl<'a> TraceSession<'a> {
    pub fn new(ring: &'a TraceRing, component: ComponentId) -> Self {
        Self { ring, component, tick: AtomicU64::new(0) }
    }

    pub fn set_tick(&self, tick: u64) { self.tick.store(tick, Ordering::Relaxed); }
    pub fn bump_tick(&self) { self.tick.fetch_add(1, Ordering::Relaxed); }

    pub fn error(&self, msg: &str) { self.emit(TraceLevel::Error, msg); }
    pub fn warn(&self,  msg: &str) { self.emit(TraceLevel::Warn,  msg); }
    pub fn info(&self,  msg: &str) { self.emit(TraceLevel::Info,  msg); }
    pub fn debug(&self, msg: &str) { self.emit(TraceLevel::Debug, msg); }
    pub fn trace(&self, msg: &str) { self.emit(TraceLevel::Trace, msg); }

    fn emit(&self, level: TraceLevel, msg: &str) {
        let tick = self.tick.load(Ordering::Relaxed);
        self.ring.emit(tick, self.component, level, msg);
    }

    pub fn collect(&self, n: usize) -> ExofsResult<Vec<TraceEvent>> {
        let filter = TraceFilter::for_component(self.component);
        self.ring.last_n_filtered(n, &filter)
    }
}

// ─── TraceSummary ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct TraceSummary {
    pub total_events: u64,
    pub dropped:      u64,
    pub error_count:  u64,
    pub warn_count:   u64,
    pub info_count:   u64,
    pub debug_count:  u64,
}

impl TraceSummary {
    /// Calcule le résumé depuis le ring (RECUR-01 : boucle while).
    pub fn from_ring(ring: &TraceRing) -> ExofsResult<Self> {
        let count = ring.count.load(Ordering::Relaxed) as usize;
        let mut s = Self {
            total_events: ring.count(),
            dropped:      ring.dropped(),
            error_count:  0, warn_count: 0, info_count: 0, debug_count: 0,
        };
        let head = ring.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < count {
            let idx = (head.wrapping_add(TRACE_RING_SIZE).wrapping_sub(i + 1)) % TRACE_RING_SIZE;
            let evt = unsafe { (*ring.events.get())[idx] };
            match TraceLevel::from_u8(evt.level) {
                TraceLevel::Error => s.error_count = s.error_count.saturating_add(1),
                TraceLevel::Warn  => s.warn_count  = s.warn_count.saturating_add(1),
                TraceLevel::Info  => s.info_count  = s.info_count.saturating_add(1),
                TraceLevel::Debug | TraceLevel::Trace => s.debug_count = s.debug_count.saturating_add(1),
                _ => {}
            }
            i = i.wrapping_add(1);
        }
        Ok(s)
    }

    /// Taux d'erreurs en ‰.
    pub fn error_ratio_ppt(&self) -> u64 {
        if self.total_events == 0 { return 0; }
        self.error_count.saturating_mul(1000)
            .checked_div(self.total_events).unwrap_or(0)
    }

    /// Vrai si des erreurs ont été tracées.
    pub fn has_errors(&self) -> bool { self.error_count > 0 }

    /// Vrai si le ring a perdu des événements.
    pub fn has_dropped(&self) -> bool { self.dropped > 0 }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ring() -> TraceRing { TraceRing::new_const() }

    #[test]
    fn test_trace_level_order() {
        assert!(TraceLevel::Error < TraceLevel::Warn);
        assert!(TraceLevel::Trace > TraceLevel::Info);
        assert!(TraceLevel::Off < TraceLevel::Error);
    }

    #[test]
    fn test_trace_level_from_u8() {
        assert_eq!(TraceLevel::from_u8(2), TraceLevel::Warn);
        assert_eq!(TraceLevel::from_u8(99), TraceLevel::Off);
    }

    #[test]
    fn test_event_new_and_msg() {
        let evt = TraceEvent::new(42, ComponentId::IO, TraceLevel::Info, "hello");
        assert_eq!(evt.tick, 42);
        assert_eq!(evt.component, ComponentId::IO.0);
        assert_eq!(evt.level, TraceLevel::Info as u8);
        let v = evt.msg_to_vec().expect("ok");
        assert_eq!(&v, b"hello");
    }

    #[test]
    fn test_event_zeroed() {
        let evt = TraceEvent::zeroed();
        assert!(evt.is_empty());
    }

    #[test]
    fn test_filter_matches() {
        let f = TraceFilter::errors();
        let info = TraceEvent::new(1, ComponentId::CORE, TraceLevel::Info, "x");
        let err  = TraceEvent::new(2, ComponentId::CORE, TraceLevel::Error, "y");
        assert!(!f.matches(&info));
        assert!(f.matches(&err));
    }

    #[test]
    fn test_ring_push_and_latest() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Trace);
        r.emit(1, ComponentId::IO, TraceLevel::Info, "test");
        let l = r.latest().expect("some");
        assert_eq!(l.tick, 1);
    }

    #[test]
    fn test_ring_filtered() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Trace);
        r.emit(1, ComponentId::IO,   TraceLevel::Error, "err");
        r.emit(2, ComponentId::CORE, TraceLevel::Info,  "info");
        let f = TraceFilter::for_component(ComponentId::IO);
        let v = r.last_n_filtered(10, &f).expect("ok");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].component, ComponentId::IO.0);
    }

    #[test]
    fn test_ring_below_level_ignored() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Error);
        r.emit(1, ComponentId::CORE, TraceLevel::Debug, "debug msg");
        assert_eq!(r.count(), 0);
    }

    #[test]
    fn test_ring_reset() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Trace);
        r.emit(1, ComponentId::CORE, TraceLevel::Info, "x");
        r.reset();
        assert_eq!(r.count(), 0);
        assert!(r.latest().is_none());
    }

    #[test]
    fn test_session_emit_and_collect() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Trace);
        let sess = TraceSession::new(&r, ComponentId::GC);
        sess.info("gc started");
        sess.warn("gc slow");
        let v = sess.collect(10).expect("ok");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_session_bump_tick() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Trace);
        let sess = TraceSession::new(&r, ComponentId::CACHE);
        sess.bump_tick();
        sess.info("step");
        let l = r.latest().expect("latest");
        assert_eq!(l.tick, 1);
    }

    #[test]
    fn test_summary_from_ring() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Trace);
        r.emit(1, ComponentId::CORE, TraceLevel::Error, "e1");
        r.emit(2, ComponentId::CORE, TraceLevel::Error, "e2");
        r.emit(3, ComponentId::CORE, TraceLevel::Info,  "i1");
        let s = TraceSummary::from_ring(&r).expect("ok");
        assert_eq!(s.error_count, 2);
        assert_eq!(s.info_count, 1);
        assert!(s.has_errors());
    }

    #[test]
    fn test_summary_error_ratio() {
        let r = make_ring();
        r.set_min_level(TraceLevel::Trace);
        r.emit(1, ComponentId::CORE, TraceLevel::Error, "e");
        r.emit(2, ComponentId::CORE, TraceLevel::Info,  "i");
        r.emit(3, ComponentId::CORE, TraceLevel::Info,  "i");
        r.emit(4, ComponentId::CORE, TraceLevel::Info,  "i");
        let s = TraceSummary::from_ring(&r).expect("ok");
        assert_eq!(s.error_ratio_ppt(), 250); // 1/4 × 1000
    }

    #[test]
    fn test_component_id_name() {
        assert_eq!(ComponentId::IO.name(), "IO");
        assert_eq!(ComponentId::SECURITY.name(), "SECURITY");
    }
}
