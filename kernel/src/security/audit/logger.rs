// kernel/src/security/audit/logger.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Audit Logger — Journal d'audit du kernel (syscalls, violations, events)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Ring buffer lock-free de 65536 entrées (1M de mémoire)
//   • Chaque entrée : 16 bytes minimum (catégorie, tid, timestamp, data)
//   • Flush vers userspace via interface mémoire partagée (zero-copy)
//   • Distinct du tcb/audit.rs (TCB ring buffer = sécurité bas niveau)
//     Ici : audit de politique de haut niveau (LSM-like)
//
// RÈGLE AUDIT-01 : log_event() doit être non-bloquant (ISR-safe) — spin uniquement.
// RÈGLE AUDIT-02 : Les événements critiques (SECVIOL) ne peuvent pas être filtrés.
// RÈGLE AUDIT-03 : Le buffer plein → événements les plus anciens écrasés (ring).
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Types d'événements
// ─────────────────────────────────────────────────────────────────────────────

/// Catégorie d'événement d'audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditCategory {
    /// Appel système.
    Syscall        = 0x01,
    /// Violation de politique de sécurité.
    SecurityViolation = 0x02,
    /// Opération sur les capabilities.
    Capability     = 0x03,
    /// Accès au système de fichiers.
    FileAccess     = 0x04,
    /// Opération réseau.
    Network        = 0x05,
    /// Création/terminaison de processus.
    Process        = 0x06,
    /// Opération IPC.
    Ipc            = 0x07,
    /// Événement d'authentification.
    Auth           = 0x08,
    /// Violation crypto (signature invalide, etc.).
    Crypto         = 0x09,
    /// Événement de boot.
    Boot           = 0x0A,
    /// Événement divers.
    Other          = 0xFF,
}

/// Résultat d'une opération (pour les événements de type Syscall).
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum AuditOutcome {
    Allow   = 0,
    Deny    = 1,
    Error   = 2,
    Kill    = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entrée du ring buffer d'audit
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'audit (16 bytes, cache-line friendly × 4 par cache line).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct AuditRecord {
    /// Timestamp TSC (cycles CPU).
    pub timestamp:  u64,
    /// Thread ID émetteur.
    pub thread_id:  u32,
    /// Process ID émetteur.
    pub pid:        u32,
    /// Numéro de syscall (ou 0).
    pub syscall_nr: u32,
    /// Code d'erreur / résultat.
    pub result:     i32,
    /// Catégorie d'événement.
    pub category:   AuditCategory,
    /// Résultat de l'opération.
    pub outcome:    AuditOutcome,
    /// UID de l'appelant.
    pub uid:        u16,
    /// Padding pour 24 bytes totaux.
    pub _pad:       [u8; 2],
    /// Données supplémentaires (hash de path, addr, etc.).
    pub data:       [u8; 8],
}

impl AuditRecord {
    pub const fn zeroed() -> Self {
        Self {
            timestamp:  0,
            thread_id:  0,
            pid:        0,
            syscall_nr: 0,
            result:     0,
            category:   AuditCategory::Other,
            outcome:    AuditOutcome::Allow,
            uid:        0,
            _pad:       [0; 2],
            data:       [0; 8],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ring buffer
// ─────────────────────────────────────────────────────────────────────────────

/// Taille du ring buffer : 65536 entrées.
const RING_SIZE: usize = 65536;

struct AuditRing {
    buffer:   [AuditRecord; RING_SIZE],
    head:     AtomicUsize,  // Position d'écriture (produit)
    tail:     AtomicUsize,  // Position de lecture (consommateur)
    overflow: AtomicU64,
}

impl AuditRing {
    const fn new() -> Self {
        Self {
            buffer:   [AuditRecord::zeroed(); RING_SIZE],
            head:     AtomicUsize::new(0),
            tail:     AtomicUsize::new(0),
            overflow: AtomicU64::new(0),
        }
    }

    /// Écrit un événement dans le ring buffer (RÈGLE AUDIT-01 : lock-free).
    ///
    /// Implémente une politique OVERWRITE si le buffer est plein (RÈGLE AUDIT-03).
    fn push(&mut self, record: AuditRecord) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) % RING_SIZE;
        // Vérifier si on va écraser un élément non consommé
        let tail = self.tail.load(Ordering::Relaxed);
        let head = idx;
        // Distance circulaire
        let used = head.wrapping_sub(tail) % RING_SIZE;
        if used >= RING_SIZE - 1 {
            // Buffer plein : avancer le tail (écrase le plus ancien)
            self.tail.fetch_add(1, Ordering::Relaxed);
            self.overflow.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: idx < RING_SIZE par construction du modulo
        unsafe {
            let ptr = self.buffer.as_mut_ptr().add(idx);
            core::ptr::write_volatile(ptr, record);
        }
    }

    /// Lit jusqu'à `max` entrées depuis le tail, retourne le nombre lu.
    fn drain(&mut self, out: &mut [AuditRecord]) -> usize {
        let mut count = 0usize;
        while count < out.len() {
            let tail = self.tail.load(Ordering::Relaxed);
            let head = self.head.load(Ordering::Relaxed) % RING_SIZE;
            if tail % RING_SIZE == head { break; }
            let idx = tail % RING_SIZE;
            // SAFETY: idx < RING_SIZE
            out[count] = unsafe {
                core::ptr::read_volatile(self.buffer.as_ptr().add(idx))
            };
            self.tail.fetch_add(1, Ordering::Relaxed);
            count += 1;
        }
        count
    }

    fn pending(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail) % RING_SIZE
    }
}

static AUDIT_RING: spin::Mutex<AuditRing> =
    spin::Mutex::new(AuditRing::new());

// ─────────────────────────────────────────────────────────────────────────────
// Filtres
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmask des catégories activées (1 bit par AuditCategory u8).
/// Bit 1 = Syscall, bit 2 = SecurityViolation, etc.
static FILTER_MASK: AtomicU64 = AtomicU64::new(!0u64); // Tout activé par défaut

static EVENTS_LOGGED:    AtomicU64 = AtomicU64::new(0);
static EVENTS_DROPPED:   AtomicU64 = AtomicU64::new(0);
static EVENTS_CRITICAL:  AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaire TSC
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
        );
    }
    (hi as u64) << 32 | lo as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistre un événement d'audit.
///
/// RÈGLE AUDIT-01 : Non-bloquant (spin lock court, pas d'allocation).
/// RÈGLE AUDIT-02 : SecurityViolation ne peut pas être filtrée.
pub fn log_event(
    category:   AuditCategory,
    pid:        u32,
    thread_id:  u32,
    uid:        u16,
    syscall_nr: u32,
    result:     i32,
    outcome:    AuditOutcome,
    data:       [u8; 8],
) {
    let critical = category == AuditCategory::SecurityViolation;

    // Vérifier le filtre (RÈGLE AUDIT-02 : SecurityViolation toujours loguée)
    let mask = FILTER_MASK.load(Ordering::Relaxed);
    let cat_bit = 1u64 << (category as u8);
    if !critical && (mask & cat_bit == 0) {
        EVENTS_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let record = AuditRecord {
        timestamp:  rdtsc(),
        thread_id,
        pid,
        syscall_nr,
        result,
        category,
        outcome,
        uid,
        _pad:       [0; 2],
        data,
    };

    AUDIT_RING.lock().push(record);
    EVENTS_LOGGED.fetch_add(1, Ordering::Relaxed);
    if critical {
        EVENTS_CRITICAL.fetch_add(1, Ordering::Relaxed);
    }
}

/// Enregistre une violation de sécurité (raccourci pour RÈGLE AUDIT-02).
pub fn log_security_violation(
    pid:       u32,
    tid:       u32,
    uid:       u16,
    syscall_nr: u32,
    data:      [u8; 8],
) {
    log_event(
        AuditCategory::SecurityViolation,
        pid, tid, uid,
        syscall_nr,
        -1,
        AuditOutcome::Deny,
        data,
    );
}

/// Flush les événements en attente dans `out`.
///
/// Retourne le nombre d'événements copiés.
pub fn flush_to_userspace(out: &mut [AuditRecord]) -> usize {
    AUDIT_RING.lock().drain(out)
}

/// Nombre d'événements en attente de lecture.
pub fn pending_events() -> usize {
    AUDIT_RING.lock().pending()
}

/// Active/désactive une catégorie d'audit (sauf SecurityViolation, toujours actée).
pub fn set_filter(category: AuditCategory, enabled: bool) {
    if category == AuditCategory::SecurityViolation { return; } // RÈGLE AUDIT-02
    let bit = 1u64 << (category as u8);
    if enabled {
        FILTER_MASK.fetch_or(bit, Ordering::Relaxed);
    } else {
        FILTER_MASK.fetch_and(!bit, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AuditLoggerStats {
    pub events_logged:   u64,
    pub events_dropped:  u64,
    pub events_critical: u64,
    pub overflow_count:  u64,
    pub pending:         usize,
}

pub fn audit_logger_stats() -> AuditLoggerStats {
    AuditLoggerStats {
        events_logged:   EVENTS_LOGGED.load(Ordering::Relaxed),
        events_dropped:  EVENTS_DROPPED.load(Ordering::Relaxed),
        events_critical: EVENTS_CRITICAL.load(Ordering::Relaxed),
        overflow_count:  AUDIT_RING.lock().overflow.load(Ordering::Relaxed),
        pending:         AUDIT_RING.lock().pending(),
    }
}
