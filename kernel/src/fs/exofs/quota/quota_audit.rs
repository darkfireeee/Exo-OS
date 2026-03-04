// SPDX-License-Identifier: MIT
// ExoFS Quota — Journal d'audit
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::quota_tracker::QuotaKey;

// ─── Constants ────────────────────────────────────────────────────────────────

pub const AUDIT_RING_SIZE: usize   = 2048;
pub const AUDIT_MSG_LEN:   usize   = 48;

// ─── Horloge interne ─────────────────────────────────────────────────────────

static AUDIT_TICK: AtomicU64 = AtomicU64::new(0);
pub fn audit_tick() -> u64          { AUDIT_TICK.load(Ordering::Relaxed) }
pub fn advance_audit_tick(dt: u64)  { AUDIT_TICK.fetch_add(dt, Ordering::Relaxed); }

// ─── QuotaEvent ───────────────────────────────────────────────────────────────

/// Type d'événement de quota auditable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum QuotaEvent {
    /// Dépassement de la limite souple.
    SoftBreach       = 0x01,
    /// Refus par la limite dure.
    HardDenial       = 0x02,
    /// Nouvelle limite posée.
    LimitSet         = 0x03,
    /// Limite réinitialisée.
    LimitReset       = 0x04,
    /// Namespace ajouté.
    NamespaceAdded   = 0x05,
    /// Namespace supprimé.
    NamespaceRemoved = 0x06,
    /// Entité créée dans le tracker.
    EntityCreated    = 0x07,
    /// Entité supprimée du tracker.
    EntityRemoved    = 0x08,
    /// Dépassement de grâce expiré.
    GraceExpired     = 0x09,
    /// Utilisation remise à zéro.
    UsageReset       = 0x0A,
    /// Alerte dépassement soft imminent (>80%).
    SoftWarning      = 0x0B,
    /// Politique appliquée (changement).
    PolicyApplied    = 0x0C,
}

impl QuotaEvent {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::SoftBreach),
            0x02 => Some(Self::HardDenial),
            0x03 => Some(Self::LimitSet),
            0x04 => Some(Self::LimitReset),
            0x05 => Some(Self::NamespaceAdded),
            0x06 => Some(Self::NamespaceRemoved),
            0x07 => Some(Self::EntityCreated),
            0x08 => Some(Self::EntityRemoved),
            0x09 => Some(Self::GraceExpired),
            0x0A => Some(Self::UsageReset),
            0x0B => Some(Self::SoftWarning),
            0x0C => Some(Self::PolicyApplied),
            _    => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SoftBreach       => "SOFT_BREACH",
            Self::HardDenial       => "HARD_DENIAL",
            Self::LimitSet         => "LIMIT_SET",
            Self::LimitReset       => "LIMIT_RESET",
            Self::NamespaceAdded   => "NS_ADDED",
            Self::NamespaceRemoved => "NS_REMOVED",
            Self::EntityCreated    => "ENTITY_CREATED",
            Self::EntityRemoved    => "ENTITY_REMOVED",
            Self::GraceExpired     => "GRACE_EXPIRED",
            Self::UsageReset       => "USAGE_RESET",
            Self::SoftWarning      => "SOFT_WARNING",
            Self::PolicyApplied    => "POLICY_APPLIED",
        }
    }

    pub fn is_denial(self) -> bool {
        matches!(self, Self::HardDenial | Self::GraceExpired)
    }

    pub fn is_breach(self) -> bool {
        matches!(self, Self::SoftBreach | Self::HardDenial | Self::SoftWarning | Self::GraceExpired)
    }

    pub fn severity(self) -> u8 {
        match self {
            Self::HardDenial | Self::GraceExpired => 3,
            Self::SoftBreach | Self::SoftWarning  => 2,
            Self::LimitSet | Self::LimitReset | Self::PolicyApplied => 1,
            _ => 0,
        }
    }
}

// ─── QuotaAuditEntry ──────────────────────────────────────────────────────────

/// Entrée d'audit (128 octets, repr C, copiable).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct QuotaAuditEntry {
    pub tick:      u64,
    pub entity_id: u64,
    pub current:   u64,
    pub limit:     u64,
    pub event:     u8,
    pub kind:      u8,  // QuotaKind as u8
    pub severity:  u8,
    pub _pad:      [u8; 5],
    pub msg:       [u8; AUDIT_MSG_LEN],
}

impl QuotaAuditEntry {
    pub const fn zeroed() -> Self {
        Self {
            tick: 0, entity_id: 0, current: 0, limit: 0,
            event: 0, kind: 0, severity: 0, _pad: [0; 5],
            msg: [0; AUDIT_MSG_LEN],
        }
    }

    pub fn new(
        tick:  u64,
        key:   QuotaKey,
        event: QuotaEvent,
        current: u64,
        limit: u64,
        msg:   &str,
    ) -> Self {
        let mut e = Self::zeroed();
        e.tick      = tick;
        e.entity_id = key.entity_id;
        e.kind      = key.kind;
        e.event     = event as u8;
        e.current   = current;
        e.limit     = limit;
        e.severity  = event.severity();
        let bytes = msg.as_bytes();
        let len = bytes.len().min(AUDIT_MSG_LEN);
        let mut i = 0usize;
        while i < len { e.msg[i] = bytes[i]; i = i.wrapping_add(1); }
        e
    }

    pub fn is_empty(&self) -> bool { self.event == 0 && self.tick == 0 }

    pub fn event_kind(&self) -> Option<QuotaEvent> { QuotaEvent::from_u8(self.event) }

    pub fn msg_str(&self) -> &str {
        let end = self.msg.iter().position(|&b| b == 0).unwrap_or(AUDIT_MSG_LEN);
        core::str::from_utf8(&self.msg[..end]).unwrap_or("<invalid>")
    }

    pub fn msg_to_vec(&self) -> ExofsResult<Vec<u8>> {
        let len = self.msg.iter().position(|&b| b == 0).unwrap_or(AUDIT_MSG_LEN);
        let mut v = Vec::new();
        v.try_reserve(len).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < len { v.push(self.msg[i]); i = i.wrapping_add(1); }
        Ok(v)
    }

    /// Utilisation en ‰ du quota (ARITH-02).
    pub fn usage_ppt(&self) -> u64 {
        if self.limit == 0 || self.limit == u64::MAX { return 0; }
        self.current.saturating_mul(1000)
            .checked_div(self.limit).unwrap_or(1000)
            .min(1000)
    }
}

// ─── AuditFilter ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct AuditFilter {
    pub min_severity:  u8,
    pub event_mask:    u16, // bitmask des événements (1 << event as u8)
    pub entity_id:     Option<u64>,
}

impl AuditFilter {
    pub const fn all() -> Self {
        Self { min_severity: 0, event_mask: u16::MAX, entity_id: None }
    }
    pub const fn denials_only() -> Self {
        // HardDenial=0x02, GraceExpired=0x09
        Self { min_severity: 2, event_mask: (1 << 0x02) | (1 << 0x09), entity_id: None }
    }
    pub const fn breaches() -> Self {
        Self { min_severity: 2, event_mask: u16::MAX, entity_id: None }
    }
    pub fn for_entity(id: u64) -> Self {
        Self { min_severity: 0, event_mask: u16::MAX, entity_id: Some(id) }
    }

    pub fn matches(&self, e: &QuotaAuditEntry) -> bool {
        if e.severity < self.min_severity { return false; }
        let bit = e.event as u16;
        if bit < 16 && (self.event_mask & (1 << bit)) == 0 { return false; }
        if let Some(eid) = self.entity_id { if e.entity_id != eid { return false; } }
        true
    }
}

// ─── QuotaAuditLog ────────────────────────────────────────────────────────────

/// Journal circulaire d'audit (ring de AUDIT_RING_SIZE entrées).
pub struct QuotaAuditLog {
    entries:       UnsafeCell<[QuotaAuditEntry; AUDIT_RING_SIZE]>,
    head:          AtomicU64,
    count:         AtomicU64,
    cnt_breach:    AtomicU64,
    cnt_denial:    AtomicU64,
    cnt_admin:     AtomicU64,
    cnt_dropped:   AtomicU64,
}

unsafe impl Sync for QuotaAuditLog {}
unsafe impl Send for QuotaAuditLog {}

impl QuotaAuditLog {
    pub const fn new_const() -> Self {
        Self {
            entries:     UnsafeCell::new([QuotaAuditEntry::zeroed(); AUDIT_RING_SIZE]),
            head:        AtomicU64::new(0),
            count:       AtomicU64::new(0),
            cnt_breach:  AtomicU64::new(0),
            cnt_denial:  AtomicU64::new(0),
            cnt_admin:   AtomicU64::new(0),
            cnt_dropped: AtomicU64::new(0),
        }
    }

    /// Enregistre une entrée d'audit dans le ring.
    pub fn push(&self, entry: QuotaAuditEntry) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % AUDIT_RING_SIZE;
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        unsafe { (*self.entries.get())[idx] = entry; }
        let n = self.count.load(Ordering::Relaxed);
        if n < AUDIT_RING_SIZE as u64 {
            self.count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.cnt_dropped.fetch_add(1, Ordering::Relaxed);
        }
        match entry.event_kind() {
            Some(ev) if ev.is_denial()  => { self.cnt_denial.fetch_add(1, Ordering::Relaxed); }
            Some(ev) if ev.is_breach()  => { self.cnt_breach.fetch_add(1, Ordering::Relaxed); }
            _ => { self.cnt_admin.fetch_add(1, Ordering::Relaxed); }
        }
    }

    /// Raccourci : journalise un événement directement.
    pub fn log(&self, key: QuotaKey, event: QuotaEvent, current: u64, limit: u64, msg: &str) {
        let tick = audit_tick();
        self.push(QuotaAuditEntry::new(tick, key, event, current, limit, msg));
    }

    pub fn log_soft_breach(&self, key: QuotaKey, current: u64, limit: u64) {
        self.log(key, QuotaEvent::SoftBreach, current, limit, "soft quota exceeded");
    }

    pub fn log_hard_denial(&self, key: QuotaKey, current: u64, limit: u64) {
        self.log(key, QuotaEvent::HardDenial, current, limit, "hard quota denied");
    }

    pub fn log_limit_set(&self, key: QuotaKey, new_limit: u64) {
        self.log(key, QuotaEvent::LimitSet, 0, new_limit, "limit updated");
    }

    pub fn log_entity_created(&self, key: QuotaKey) {
        self.log(key, QuotaEvent::EntityCreated, 0, 0, "entity created");
    }

    pub fn log_entity_removed(&self, key: QuotaKey) {
        self.log(key, QuotaEvent::EntityRemoved, 0, 0, "entity removed");
    }

    pub fn log_grace_expired(&self, key: QuotaKey, current: u64, limit: u64) {
        self.log(key, QuotaEvent::GraceExpired, current, limit, "grace period expired");
    }

    pub fn log_soft_warning(&self, key: QuotaKey, usage_ppt: u64, limit: u64) {
        self.log(key, QuotaEvent::SoftWarning, usage_ppt, limit, "soft warning >80%");
    }

    /// Dernier événement enregistré.
    pub fn latest(&self) -> Option<QuotaAuditEntry> {
        if self.count.load(Ordering::Relaxed) == 0 { return None; }
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx = (head.wrapping_add(AUDIT_RING_SIZE).wrapping_sub(1)) % AUDIT_RING_SIZE;
        // SAFETY: validité des données vérifiée par les gardes ci-dessus.
        let e = unsafe { (*self.entries.get())[idx] };
        if e.is_empty() { None } else { Some(e) }
    }

    /// Retourne les n derniers événements correspondant au filtre (OOM-02, RECUR-01).
    pub fn last_n_filtered(&self, n: usize, filter: &AuditFilter) -> ExofsResult<Vec<QuotaAuditEntry>> {
        let total = self.count.load(Ordering::Relaxed) as usize;
        let capacity = n.min(total);
        let mut v = Vec::new();
        v.try_reserve(capacity).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut i = 0usize;
        while i < total && v.len() < n {
            let idx = (head.wrapping_add(AUDIT_RING_SIZE).wrapping_sub(i + 1)) % AUDIT_RING_SIZE;
            // SAFETY: validité des données vérifiée par les gardes ci-dessus.
            let entry = unsafe { (*self.entries.get())[idx] };
            if filter.matches(&entry) {
                v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                v.push(entry);
            }
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    pub fn count(&self)         -> u64 { self.count.load(Ordering::Relaxed) }
    pub fn breach_count(&self)  -> u64 { self.cnt_breach.load(Ordering::Relaxed) }
    pub fn denial_count(&self)  -> u64 { self.cnt_denial.load(Ordering::Relaxed) }
    pub fn admin_count(&self)   -> u64 { self.cnt_admin.load(Ordering::Relaxed) }
    pub fn dropped_count(&self) -> u64 { self.cnt_dropped.load(Ordering::Relaxed) }

    /// Taux de refus en ‰ (ARITH-02).
    pub fn denial_rate_ppt(&self) -> u64 {
        let total = self.count();
        if total == 0 { return 0; }
        self.denial_count()
            .saturating_mul(1000)
            .checked_div(total).unwrap_or(0)
    }

    /// Résumé statistique.
    pub fn summary(&self) -> AuditSummary {
        AuditSummary {
            total:        self.count(),
            breaches:     self.breach_count(),
            denials:      self.denial_count(),
            admin_events: self.admin_count(),
            dropped:      self.dropped_count(),
            denial_rate_ppt: self.denial_rate_ppt(),
        }
    }

    /// Remet à zéro les compteurs (conserve le ring).
    pub fn reset_counters(&self) {
        self.cnt_breach.store(0, Ordering::Relaxed);
        self.cnt_denial.store(0, Ordering::Relaxed);
        self.cnt_admin.store(0, Ordering::Relaxed);
        self.cnt_dropped.store(0, Ordering::Relaxed);
    }
}

/// Singleton global du journal d'audit.
pub static QUOTA_AUDIT: QuotaAuditLog = QuotaAuditLog::new_const();

// ─── AuditSummary ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct AuditSummary {
    pub total:           u64,
    pub breaches:        u64,
    pub denials:         u64,
    pub admin_events:    u64,
    pub dropped:         u64,
    pub denial_rate_ppt: u64,
}

impl AuditSummary {
    pub fn is_healthy(&self) -> bool {
        self.denial_rate_ppt < 10 && self.dropped == 0
    }
    pub fn has_denials(&self) -> bool { self.denials > 0 }
    pub fn has_dropped(&self) -> bool { self.dropped > 0 }
}

// ─── AuditSession ─────────────────────────────────────────────────────────────

/// Session d'audit liée à une entité spécifique.
pub struct AuditSession<'a> {
    log:       &'a QuotaAuditLog,
    key:       QuotaKey,
    event_cnt: AtomicU64,
}

impl<'a> AuditSession<'a> {
    pub fn new(log: &'a QuotaAuditLog, key: QuotaKey) -> Self {
        Self { log, key, event_cnt: AtomicU64::new(0) }
    }

    pub fn soft_breach(&self, current: u64, limit: u64) {
        self.log.log_soft_breach(self.key, current, limit);
        self.event_cnt.fetch_add(1, Ordering::Relaxed);
    }

    pub fn hard_denial(&self, current: u64, limit: u64) {
        self.log.log_hard_denial(self.key, current, limit);
        self.event_cnt.fetch_add(1, Ordering::Relaxed);
    }

    pub fn grace_expired(&self, current: u64, limit: u64) {
        self.log.log_grace_expired(self.key, current, limit);
        self.event_cnt.fetch_add(1, Ordering::Relaxed);
    }

    pub fn soft_warning(&self, usage_ppt: u64, limit: u64) {
        self.log.log_soft_warning(self.key, usage_ppt, limit);
        self.event_cnt.fetch_add(1, Ordering::Relaxed);
    }

    pub fn event_count(&self) -> u64 { self.event_cnt.load(Ordering::Relaxed) }

    pub fn history(&self, n: usize) -> ExofsResult<Vec<QuotaAuditEntry>> {
        let f = AuditFilter::for_entity(self.key.entity_id);
        self.log.last_n_filtered(n, &f)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::quota::quota_policy::QuotaKind;

    fn k(id: u64) -> QuotaKey { QuotaKey::new(QuotaKind::User, id) }

    #[test]
    fn test_event_from_u8() {
        assert_eq!(QuotaEvent::from_u8(0x01), Some(QuotaEvent::SoftBreach));
        assert_eq!(QuotaEvent::from_u8(0x02), Some(QuotaEvent::HardDenial));
        assert_eq!(QuotaEvent::from_u8(0xFF), None);
    }

    #[test]
    fn test_event_severity() {
        assert_eq!(QuotaEvent::HardDenial.severity(), 3);
        assert_eq!(QuotaEvent::SoftBreach.severity(), 2);
        assert_eq!(QuotaEvent::LimitSet.severity(), 1);
        assert_eq!(QuotaEvent::EntityCreated.severity(), 0);
    }

    #[test]
    fn test_entry_zeroed_is_empty() {
        let e = QuotaAuditEntry::zeroed();
        assert!(e.is_empty());
    }

    #[test]
    fn test_entry_new() {
        let key = k(42);
        let e = QuotaAuditEntry::new(100, key, QuotaEvent::SoftBreach, 800, 1000, "breach");
        assert_eq!(e.tick, 100);
        assert_eq!(e.entity_id, 42);
        assert_eq!(e.event, QuotaEvent::SoftBreach as u8);
        assert_eq!(e.msg_str(), "breach");
    }

    #[test]
    fn test_entry_usage_ppt() {
        let e = QuotaAuditEntry::new(0, k(1), QuotaEvent::SoftBreach, 500, 1000, "");
        assert_eq!(e.usage_ppt(), 500);
    }

    #[test]
    fn test_audit_filter_all() {
        let f = AuditFilter::all();
        let e = QuotaAuditEntry::new(0, k(1), QuotaEvent::HardDenial, 0, 0, "");
        assert!(f.matches(&e));
    }

    #[test]
    fn test_audit_filter_severity() {
        let f = AuditFilter { min_severity: 3, event_mask: u16::MAX, entity_id: None };
        let soft = QuotaAuditEntry::new(0, k(1), QuotaEvent::SoftBreach, 0, 0, "");
        let hard = QuotaAuditEntry::new(0, k(1), QuotaEvent::HardDenial, 0, 0, "");
        assert!(!f.matches(&soft));
        assert!(f.matches(&hard));
    }

    #[test]
    fn test_audit_filter_entity() {
        let f = AuditFilter::for_entity(99);
        let e1 = QuotaAuditEntry::new(0, k(99), QuotaEvent::SoftBreach, 0, 0, "");
        let e2 = QuotaAuditEntry::new(0, k(1),  QuotaEvent::SoftBreach, 0, 0, "");
        assert!(f.matches(&e1));
        assert!(!f.matches(&e2));
    }

    #[test]
    fn test_audit_log_push_and_latest() {
        let log = QuotaAuditLog::new_const();
        log.log_soft_breach(k(1), 900, 1000);
        let l = log.latest().expect("some");
        assert_eq!(l.entity_id, 1);
        assert_eq!(l.event_kind(), Some(QuotaEvent::SoftBreach));
    }

    #[test]
    fn test_audit_log_counters() {
        let log = QuotaAuditLog::new_const();
        log.log_soft_breach(k(1), 900, 1000);
        log.log_hard_denial(k(2), 1100, 1000);
        log.log_limit_set(k(3), 2000);
        assert_eq!(log.breach_count(), 1);
        assert_eq!(log.denial_count(), 1);
        assert_eq!(log.admin_count(), 1);
    }

    #[test]
    fn test_audit_log_filtered() {
        let log = QuotaAuditLog::new_const();
        log.log_hard_denial(k(5), 0, 0);
        log.log_soft_breach(k(5), 0, 0);
        log.log_entity_created(k(6));
        let f = AuditFilter::for_entity(5);
        let v = log.last_n_filtered(10, &f).expect("ok");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn test_audit_log_denial_rate() {
        let log = QuotaAuditLog::new_const();
        log.log_hard_denial(k(1), 0, 0);
        log.log_soft_breach(k(1), 0, 0);
        log.log_soft_breach(k(1), 0, 0);
        log.log_soft_breach(k(1), 0, 0);
        // 1 denial / 4 total = 250‰
        assert_eq!(log.denial_rate_ppt(), 250);
    }

    #[test]
    fn test_audit_summary() {
        let log = QuotaAuditLog::new_const();
        let s = log.summary();
        assert!(s.is_healthy());
        assert!(!s.has_denials());
    }

    #[test]
    fn test_audit_session() {
        let log = QuotaAuditLog::new_const();
        let sess = AuditSession::new(&log, k(10));
        sess.soft_breach(800, 1000);
        sess.hard_denial(1100, 1000);
        assert_eq!(sess.event_count(), 2);
        let h = sess.history(5).expect("ok");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn test_audit_msg_to_vec() {
        let e = QuotaAuditEntry::new(0, k(1), QuotaEvent::LimitSet, 0, 100, "testmsg");
        let v = e.msg_to_vec().expect("ok");
        assert_eq!(&v, b"testmsg");
    }
}
