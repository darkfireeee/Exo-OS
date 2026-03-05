// kernel/src/process/signal/queue.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// File d'attente de signaux POSIX.1b (RT signals 32-63) — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════
//
// POSIX.1b garantit qu'au moins une occurrence de chaque RT signal peut être
// mis en file. On utilise un tableau fixe par signal (max SIGQUEUE_DEPTH = 32
// entrées) + compteur atomique, sans alloc dynamique.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use core::cell::UnsafeCell;

/// Profondeur maximale de la file par RT signal.
pub const SIGQUEUE_DEPTH: usize = 32;
/// Nombre de signaux RT.
const RT_NSIG: usize = 32; // 32..63

/// Compteur global de dépassement de capacité (statistiques).
pub static SIGQUEUE_OVERFLOW: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// SigInfo — informations attachées à un signal (siginfo_t)
// ──────────────────────────────────────────────────═──────────────────────────

/// Informations associées à un signal (équivalent siginfo_t).
#[derive(Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct SigInfo {
    /// Numéro du signal (1..63).
    pub signo:  u32,
    /// Code du signal (SI_USER, SI_KERNEL, SI_QUEUE...).
    pub code:   i32,
    /// PID de l'expéditeur (0 si noyau).
    pub sender_pid: u32,
    /// UID de l'expéditeur.
    pub sender_uid: u32,
    /// Valeur si POSIX.1b sigqueue() (entier).
    pub value_int: i32,
    /// Valeur si POSIX.1b sigqueue() (pointeur, représenté en u64).
    pub value_ptr: u64,
    // si code=SIGFPE/SIGSEGV : adresse fautive
    pub fault_addr: u64,
}

impl SigInfo {
    pub const SI_USER:    i32 = 0;
    pub const SI_KERNEL:  i32 = 0x80;
    pub const SI_QUEUE:   i32 = -1;
    pub const SI_TIMER:   i32 = -2;
    pub const SI_MESGQ:   i32 = -3;
    pub const SI_ASYNCIO: i32 = -4;

    /// Crée un SigInfo pour kill(2).
    #[inline]
    pub fn from_kill(sig: u8, sender_pid: u32, sender_uid: u32) -> Self {
        Self {
            signo: sig as u32,
            code: Self::SI_USER,
            sender_pid,
            sender_uid,
            ..Default::default()
        }
    }

    /// Crée un SigInfo pour kill interne noyau.
    #[inline]
    pub fn kernel(sig: u8) -> Self {
        Self {
            signo: sig as u32,
            code: Self::SI_KERNEL,
            ..Default::default()
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SigQueue — file de signaux standard (un bit par signal)
// ─────────────────────────────────────────────────────────────────────────────

/// File de signaux standard (signaux 1..31).
/// Un bit par signal ; mutiples envois du même signal = 1 livraison.
#[repr(C)]
pub struct SigQueue {
    /// Bitmap de signaux en attente (bit i = signal i+1 est pending).
    pub pending: AtomicU64,
}

impl SigQueue {
    pub const fn new() -> Self {
        Self { pending: AtomicU64::new(0) }
    }

    /// Met un signal en file.
    #[inline(always)]
    pub fn enqueue(&self, sig: u8) {
        if sig == 0 || sig > 63 { return; }
        self.pending.fetch_or(1u64 << (sig - 1), Ordering::AcqRel);
    }

    /// Défile le premier signal non-bloqué.
    /// Retourne (signo, SigInfo) ou None.
    pub fn dequeue(&self, mask: u64) -> Option<(u8, SigInfo)> {
        loop {
            let pending = self.pending.load(Ordering::Acquire);
            let unblocked = pending & !mask;
            if unblocked == 0 { return None; }
            let bit = unblocked.trailing_zeros() as u8;
            let old = self.pending.fetch_and(!(1u64 << bit), Ordering::AcqRel);
            if old & (1u64 << bit) != 0 {
                return Some((bit + 1, SigInfo::kernel(bit + 1)));
            }
            // Le bit a été effacé par un autre CPU entre les deux : réessayer.
        }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.pending.load(Ordering::Acquire) == 0
    }

    /// Signaux pendant non bloqués par `mask`.
    #[inline(always)]
    pub fn has_pending(&self, mask: u64) -> bool {
        self.pending.load(Ordering::Acquire) & !mask != 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RTSigQueue — file de signaux temps-réel (POSIX.1b, signaux 32..63)
// ─────────────────────────────────────────────────────────────────────────────

/// Une entrée de la file RT.
#[derive(Copy, Clone, Debug, Default)]
struct RTEntry {
    info:  SigInfo,
    valid: bool,
}

/// File circulaire par signal RT — SIGQUEUE_DEPTH entrées.
struct RTRing {
    /// Données brutes.
    entries: UnsafeCell<[RTEntry; SIGQUEUE_DEPTH]>,
    /// Indice de lecture.
    head:    UnsafeCell<usize>,
    /// Indice d'écriture.
    tail:    AtomicUsize,
    /// Nombre d'éléments en file.
    count:   AtomicU32,
}

// SAFETY : Exo-OS fonctionne en noyau mono-UCP avec un seul producteur/consommateur
// par file RT ; l'accès exclusif est garanti par le verrou PCB ou le contexte
// d'interruption masqué. Les champs UnsafeCell sont protégés par convention.
unsafe impl Send for RTRing {}
unsafe impl Sync for RTRing {}

impl RTRing {
    const fn new() -> Self {
        Self {
            entries: UnsafeCell::new([RTEntry { info: SigInfo { signo:0, code:0, sender_pid:0, sender_uid:0, value_int:0, value_ptr:0, fault_addr:0 }, valid:false }; SIGQUEUE_DEPTH]),
            head:    UnsafeCell::new(0),
            tail:    AtomicUsize::new(0),
            count:   AtomicU32::new(0),
        }
    }

    /// Enfile un SigInfo.
    fn push(&self, info: SigInfo) -> bool {
        if self.count.load(Ordering::Acquire) as usize >= SIGQUEUE_DEPTH {
            SIGQUEUE_OVERFLOW.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let tail = self.tail.load(Ordering::Acquire);
        // SAFETY: accès exclusif garanti par contexte d'appel (verrou PCB).
        unsafe {
            (*self.entries.get())[tail % SIGQUEUE_DEPTH] = RTEntry { info, valid: true };
        }
        self.tail.store(tail + 1, Ordering::Release);
        self.count.fetch_add(1, Ordering::AcqRel);
        true
    }

    /// Défile le prochain élément valide.
    fn pop(&self) -> Option<SigInfo> {
        if self.count.load(Ordering::Acquire) == 0 { return None; }
        // SAFETY : accès exclusif garanti par contexte d'appel.
        let info = unsafe {
            let head_ptr = self.head.get();
            let head = *head_ptr;
            let entry = &mut (*self.entries.get())[head % SIGQUEUE_DEPTH];
            if !entry.valid { return None; }
            let info = entry.info;
            entry.valid = false;
            *head_ptr += 1;
            info
        };
        self.count.fetch_sub(1, Ordering::AcqRel);
        Some(info)
    }

    fn is_empty(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }
}

/// File de signaux temps-réel pour les 32 signaux RT (SIGRTMIN..SIGRTMAX).
/// Chaque signal possède sa propre file circulaire.
pub struct RTSigQueue {
    /// Une file par signal RT (index 0 = SIGRTMIN = signal 32).
    rings: [RTRing; RT_NSIG],
    /// Bitmap indiquant quels slots ont des éléments en attente.
    pub pending_mask: AtomicU64,
}

// SAFETY : même argument que RTRing.
unsafe impl Send for RTSigQueue {}
unsafe impl Sync for RTSigQueue {}

impl RTSigQueue {
    const EMPTY_RING: RTRing = RTRing::new();

    pub const fn new() -> Self {
        Self {
            rings: [Self::EMPTY_RING; RT_NSIG],
            pending_mask: AtomicU64::new(0),
        }
    }

    /// Enfile un signal temps-réel.
    /// `sig` doit être dans [32..63].
    pub fn enqueue(&self, sig: u8, info: SigInfo) -> bool {
        let idx = sig.wrapping_sub(32) as usize;
        if idx >= RT_NSIG { return false; }
        let ok = self.rings[idx].push(info);
        if ok {
            self.pending_mask.fetch_or(1u64 << idx, Ordering::AcqRel);
        }
        ok
    }

    /// Défile le premier signal RT non-bloqué (mask = signal_mask >> 32).
    pub fn dequeue(&self, rt_mask: u32) -> Option<(u8, SigInfo)> {
        let pending = self.pending_mask.load(Ordering::Acquire) as u32;
        let unblocked = pending & !rt_mask;
        if unblocked == 0 { return None; }
        let idx = unblocked.trailing_zeros() as usize;
        let ring = &self.rings[idx];
        if let Some(info) = ring.pop() {
            if ring.is_empty() {
                self.pending_mask.fetch_and(!(1u64 << idx), Ordering::AcqRel);
            }
            Some((idx as u8 + 32, info))
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending_mask.load(Ordering::Acquire) == 0
    }

    pub fn has_pending(&self, rt_mask: u32) -> bool {
        let p = self.pending_mask.load(Ordering::Acquire) as u32;
        p & !rt_mask != 0
    }
}
