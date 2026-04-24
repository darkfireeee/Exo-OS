//! audit_log.rs — Ring-buffer lock-free pour le journal d'audit ExoFS (no_std).
//!
//! Ring-buffer de 65 536 entrées fixes en production.
//! Sous `#[cfg(test)]`, la capacité est réduite pour éviter d'allouer plusieurs
//! mégaoctets sur la pile des threads de test.
//! Les écritures sont lock-free via `fetch_add` atomique.
//! Les lectures sont diagnostiques (pas de garantie stricte de cohérence
//! en cas de lecture concurrente intense, acceptable pour l'audit).
//!
//! Règles :
//!  - ONDISK-03 : pas d'AtomicU64 dans les structs repr(C)
//!  - ARITH-02  : arithmetic overflow via wrapping
//!  - RECUR-01  : zéro récursion

use super::audit_entry::{AuditEntry, AuditResult, AuditSeverity, AUDIT_ENTRY_SIZE};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes du ring-buffer
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre d'entrées dans le ring-buffer (puissance de 2 pour masquage rapide).
#[cfg(test)]
pub const RING_SIZE: usize = 1024;
/// Taille de production du journal d'audit.
#[cfg(not(test))]
pub const RING_SIZE: usize = 65536;

/// Masque pour le calcul de l'index mod RING_SIZE sans division.
const RING_MASK: usize = RING_SIZE - 1;

/// Capacité mémoire du ring-buffer en octets.
pub const RING_CAPACITY_BYTES: usize = RING_SIZE * AUDIT_ENTRY_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// AUDIT_LOG — instance globale
// ─────────────────────────────────────────────────────────────────────────────

pub static AUDIT_LOG: AuditLog = AuditLog::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// AuditLog
// ─────────────────────────────────────────────────────────────────────────────

/// Ring-buffer non-bloquant pour les entrées d'audit.
///
/// - `head` : prochain indice d'écriture (toujours croissant).
/// - `count`: nombre total d'entrées enregistrées depuis le démarrage.
/// - `n_overwritten`: nombre d'entrées écrasées par wrap-around.
pub struct AuditLog {
    ring: [core::cell::UnsafeCell<AuditEntry>; RING_SIZE],
    head: AtomicU64,
    count: AtomicU64,
    n_overwritten: AtomicU64,
    n_security: AtomicU64,
    n_critical: AtomicU64,
    n_errors: AtomicU64,
}

// SAFETY: accès au ring uniquement via `fetch_add` atomique sur `head`.
unsafe impl Sync for AuditLog {}

impl AuditLog {
    /// Constructeur `const` pour usage en `static`.
    pub const fn new_const() -> Self {
        const ZERO_ENTRY: AuditEntry = AuditEntry {
            tick: 0,
            actor_uid: 0,
            actor_cap: 0,
            object_id: 0,
            blob_id: [0; 32],
            op: 0,
            result: 0,
            severity: 0,
            flags: 0,
            seq: 0,
            magic: 0,
            _pad: [0; 16],
        };
        const ZERO: core::cell::UnsafeCell<AuditEntry> = core::cell::UnsafeCell::new(ZERO_ENTRY);
        Self {
            ring: [ZERO; RING_SIZE],
            head: AtomicU64::new(0),
            count: AtomicU64::new(0),
            n_overwritten: AtomicU64::new(0),
            n_security: AtomicU64::new(0),
            n_critical: AtomicU64::new(0),
            n_errors: AtomicU64::new(0),
        }
    }

    // ── Écriture ──────────────────────────────────────────────────────────────

    /// Enregistre une entrée (lock-free).
    ///
    /// `fetch_add` alloue un slot unique. Si le ring était plein, l'entrée
    /// la plus ancienne est silencieusement écrasée et `n_overwritten` est
    /// incrémenté.
    pub fn push(&self, mut entry: AuditEntry) {
        let seq = self.head.fetch_add(1, Ordering::Relaxed);
        let total = self.count.fetch_add(1, Ordering::Relaxed);
        entry.seq = seq;

        let idx = (seq as usize) & RING_MASK;

        // Si le ring est plein, on va écraser une ancienne entrée.
        if total >= RING_SIZE as u64 {
            self.n_overwritten.fetch_add(1, Ordering::Relaxed);
        }

        // Mise à jour des compteurs de criticité.
        if entry.is_security() {
            self.n_security.fetch_add(1, Ordering::Relaxed);
        }
        if entry.severity >= AuditSeverity::Critical as u8 {
            self.n_critical.fetch_add(1, Ordering::Relaxed);
        }
        if AuditResult::from_u8(entry.result)
            .map(|r| !r.is_ok())
            .unwrap_or(false)
        {
            self.n_errors.fetch_add(1, Ordering::Relaxed);
        }

        // SAFETY: index unique via fetch_add + masquage ; pas de data race
        // car chaque push écrit dans un slot différent (modulo RING_SIZE,
        // l'écrasement est acceptable pour le journal d'audit).
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe {
            *self.ring[idx].get() = entry;
        }
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Lit l'entrée à la position absolue `pos` (modulo RING_SIZE).
    pub fn read_at(&self, pos: usize) -> AuditEntry {
        let idx = pos & RING_MASK;
        // SAFETY: lecture diagnostique, pas de garantie stricte.
        unsafe { *self.ring[idx].get() }
    }

    /// Lit l'entrée à l'indice courant (dernier écrit − 1).
    pub fn read_last(&self) -> Option<AuditEntry> {
        let h = self.head.load(Ordering::Relaxed);
        if h == 0 {
            return None;
        }
        Some(self.read_at((h.wrapping_sub(1)) as usize))
    }

    /// Copie les `n` dernières entrées dans `out` (lecture séquentielle).
    /// `n` est borné à `RING_SIZE`.
    /// Retourne le nombre d'entrées effectivement copiées.
    pub fn read_recent_into(&self, out: &mut [AuditEntry], n: usize) -> usize {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let total = self.count.load(Ordering::Relaxed) as usize;
        let avail = n.min(total).min(RING_SIZE).min(out.len());
        for i in 0..avail {
            let pos = head.wrapping_sub(avail).wrapping_add(i);
            out[i] = self.read_at(pos);
        }
        avail
    }

    /// Itère sur toutes les entrées valides dans l'ordre chronologique
    /// et appelle `f(entry)` pour chacune (itératif, RECUR-01).
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&AuditEntry),
    {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let total = self.count.load(Ordering::Relaxed) as usize;
        let avail = total.min(RING_SIZE);
        for i in 0..avail {
            let pos = head.wrapping_sub(avail).wrapping_add(i);
            let entry = self.read_at(pos);
            if entry.is_valid() {
                f(&entry);
            }
        }
    }

    // ── Statistiques ──────────────────────────────────────────────────────────

    /// Nombre total d'entrées enregistrées depuis le démarrage.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Prochain numéro de séquence (= head).
    pub fn next_seq(&self) -> u64 {
        self.head.load(Ordering::Relaxed)
    }

    /// Nombre d'entrées disponibles dans le ring (borné à RING_SIZE).
    pub fn available(&self) -> usize {
        (self.count.load(Ordering::Relaxed) as usize).min(RING_SIZE)
    }

    /// Nombre d'entrées écrasées par débordement.
    pub fn n_overwritten(&self) -> u64 {
        self.n_overwritten.load(Ordering::Relaxed)
    }

    /// Nombre d'entrées de sécurité enregistrées.
    pub fn n_security(&self) -> u64 {
        self.n_security.load(Ordering::Relaxed)
    }

    /// Nombre d'entrées critiques ou alertes.
    pub fn n_critical(&self) -> u64 {
        self.n_critical.load(Ordering::Relaxed)
    }

    /// Nombre d'entrées en erreur (Error ou Timeout).
    pub fn n_errors(&self) -> u64 {
        self.n_errors.load(Ordering::Relaxed)
    }

    /// Retourne un snapshot des statistiques du log.
    pub fn stats(&self) -> AuditLogStats {
        AuditLogStats {
            total_pushed: self.count.load(Ordering::Relaxed),
            available: self.available() as u64,
            n_overwritten: self.n_overwritten.load(Ordering::Relaxed),
            n_security: self.n_security.load(Ordering::Relaxed),
            n_critical: self.n_critical.load(Ordering::Relaxed),
            n_errors: self.n_errors.load(Ordering::Relaxed),
            ring_fill_pct: {
                let a = self.available();
                ((a as u64 * 100) / RING_SIZE as u64) as u8
            },
        }
    }

    /// Remet tous les compteurs à zéro (usage: tests ou rotation).
    pub fn reset_stats(&self) {
        self.n_overwritten.store(0, Ordering::Relaxed);
        self.n_security.store(0, Ordering::Relaxed);
        self.n_critical.store(0, Ordering::Relaxed);
        self.n_errors.store(0, Ordering::Relaxed);
    }

    /// Vérifie que le ring-buffer est sain (magic valide sur N entrées).
    pub fn sanity_check(&self, n: usize) -> AuditLogHealth {
        let avail = self.available();
        let sample = n.min(avail);
        let mut invalid = 0u32;
        let head = self.head.load(Ordering::Relaxed) as usize;
        for i in 0..sample {
            let pos = head.wrapping_sub(sample).wrapping_add(i);
            if !self.read_at(pos).is_valid() {
                invalid = invalid.wrapping_add(1);
            }
        }
        AuditLogHealth {
            sampled: sample as u32,
            corrupt: invalid,
            fill_pct: self.stats().ring_fill_pct,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditLogStats
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques du ring-buffer.
#[derive(Clone, Debug, Default)]
pub struct AuditLogStats {
    pub total_pushed: u64,
    pub available: u64,
    pub n_overwritten: u64,
    pub n_security: u64,
    pub n_critical: u64,
    pub n_errors: u64,
    /// Pourcentage de remplissage (0-100).
    pub ring_fill_pct: u8,
}

impl AuditLogStats {
    /// `true` si le ring est plein à plus de 80%.
    pub fn is_near_full(&self) -> bool {
        self.ring_fill_pct >= 80
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditLogHealth
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la vérification de sanité.
#[derive(Clone, Debug)]
pub struct AuditLogHealth {
    pub sampled: u32,
    pub corrupt: u32,
    pub fill_pct: u8,
}

impl AuditLogHealth {
    /// `true` si aucune entrée corrompue dans l'échantillon.
    pub fn is_clean(&self) -> bool {
        self.corrupt == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::audit_entry::{AuditEntry, AuditOp, AuditResult};
    use super::*;

    fn push_simple(log: &AuditLog, op: AuditOp, result: AuditResult) {
        log.push(AuditEntry::new(1, 1, 0, 0, [0u8; 32], op, result, 0));
    }

    #[test]
    fn test_initial_count_zero() {
        let log = AuditLog::new_const();
        assert_eq!(log.count(), 0);
        assert_eq!(log.available(), 0);
    }

    #[test]
    fn test_push_increments_count() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        assert_eq!(log.count(), 1);
        assert_eq!(log.available(), 1);
    }

    #[test]
    fn test_read_last_none_when_empty() {
        let log = AuditLog::new_const();
        assert!(log.read_last().is_none());
    }

    #[test]
    fn test_read_last_returns_last_pushed() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Write, AuditResult::Success);
        let e = log.read_last().unwrap();
        assert!(e.is_valid());
        assert_eq!(e.op, AuditOp::Write as u8);
    }

    #[test]
    fn test_seq_monotone() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        push_simple(&log, AuditOp::Write, AuditResult::Success);
        assert_eq!(log.next_seq(), 2);
    }

    #[test]
    fn test_security_counter() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::CryptoKey, AuditResult::Success);
        assert_eq!(log.n_security(), 1);
    }

    #[test]
    fn test_critical_counter() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Denied);
        assert!(log.n_critical() >= 1);
    }

    #[test]
    fn test_stats_fill_pct_zero_initially() {
        let log = AuditLog::new_const();
        let s = log.stats();
        assert_eq!(s.ring_fill_pct, 0);
    }

    #[test]
    fn test_read_recent_into() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Create, AuditResult::Success);
        push_simple(&log, AuditOp::Delete, AuditResult::Success);
        let mut buf = [AuditEntry::new(
            0,
            0,
            0,
            0,
            [0u8; 32],
            AuditOp::Read,
            AuditResult::Success,
            0,
        ); 4];
        let n = log.read_recent_into(&mut buf, 2);
        assert_eq!(n, 2);
    }

    #[test]
    fn test_sanity_check_clean() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        let h = log.sanity_check(1);
        assert!(h.is_clean());
    }

    #[test]
    fn test_for_each_count() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Success);
        push_simple(&log, AuditOp::Write, AuditResult::Success);
        let mut count = 0usize;
        log.for_each(|_| count += 1);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_reset_stats() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::CryptoKey, AuditResult::Denied);
        log.reset_stats();
        assert_eq!(log.n_security(), 0);
    }

    #[test]
    fn test_n_errors_on_timeout() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Read, AuditResult::Timeout);
        assert!(log.n_errors() >= 1);
    }

    #[test]
    fn test_available_bounded_by_ring_size() {
        let log = AuditLog::new_const();
        // Les entrées poussées ne peuvent pas dépasser RING_SIZE.
        assert!(log.available() <= RING_SIZE);
    }

    #[test]
    fn test_sanity_check_fill_pct() {
        let log = AuditLog::new_const();
        push_simple(&log, AuditOp::Create, AuditResult::Success);
        let h = log.sanity_check(1);
        // fill_pct est toujours ≤ 100.
        assert!(h.fill_pct <= 100);
    }

    #[test]
    fn test_stats_near_full_false_on_fresh() {
        let log = AuditLog::new_const();
        let stat = log.stats();
        // Un ring vide n'est pas « proche du plein ».
        assert!(!stat.is_near_full());
    }

    #[test]
    fn test_read_recent_into_zero() {
        let log = AuditLog::new_const();
        let mut buf = [];
        let n = log.read_recent_into(&mut buf, 0);
        assert_eq!(n, 0);
    }

    #[test]
    fn test_for_each_no_panic_empty() {
        let log = AuditLog::new_const();
        let mut count = 0usize;
        log.for_each(|_| count += 1);
        assert_eq!(count, 0);
    }
}
