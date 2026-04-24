// kernel/src/memory/virtual/address_space/tlb.rs
//
// Gestion du TLB (Translation Lookaside Buffer) — invalidations locales et IPI.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::memory::core::{VirtAddr, PAGE_SIZE};
use crate::memory::virt::page_table::x86_64::invlpg;

// ─────────────────────────────────────────────────────────────────────────────
// TYPE D'INVALIDATION TLB
// ─────────────────────────────────────────────────────────────────────────────

/// Type d'invalidation TLB demandée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlbFlushType {
    /// Une seule page.
    Single(VirtAddr),
    /// Plage contiguë de pages.
    Range { start: VirtAddr, end: VirtAddr },
    /// Toutes les entrées non-globales (CR3 reload).
    All,
    /// Toutes les entrées incluant les globales (CR4.PGE toggle).
    Global,
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES TLB
// ─────────────────────────────────────────────────────────────────────────────

pub struct TlbStats {
    pub single_flushes: AtomicU64,
    pub range_flushes: AtomicU64,
    pub full_flushes: AtomicU64,
    pub ipi_sent: AtomicU64,
}

impl TlbStats {
    pub const fn new() -> Self {
        TlbStats {
            single_flushes: AtomicU64::new(0),
            range_flushes: AtomicU64::new(0),
            full_flushes: AtomicU64::new(0),
            ipi_sent: AtomicU64::new(0),
        }
    }
}

pub static TLB_STATS: TlbStats = TlbStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// INVALIDATION TLB LOCALE
// ─────────────────────────────────────────────────────────────────────────────

/// Invalide une seule entrée TLB pour `addr` sur le CPU courant.
///
/// SAFETY: `addr` doit être une adresse canonique x86_64.
#[inline]
pub unsafe fn flush_single(addr: VirtAddr) {
    invlpg(addr);
    TLB_STATS.single_flushes.fetch_add(1, Ordering::Relaxed);
}

/// Invalide toutes les entrées TLB non-globales (reload de CR3).
///
/// SAFETY: La PML4 active doit rester valide après cette opération.
#[inline]
pub unsafe fn flush_all() {
    let cr3: u64;
    core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nomem, nostack));
    TLB_STATS.full_flushes.fetch_add(1, Ordering::Relaxed);
}

/// Invalide toutes les entrées TLB incluant les globales (via CR4.PGE toggle).
///
/// SAFETY: Désactive temporairement la pagination (CR4.PGE=0 → 1).
///         Doit être exécuté avec les interruptions désactivées.
#[inline]
pub unsafe fn flush_all_including_global() {
    let cr4: u64;
    core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
    core::arch::asm!("mov cr4, {}", in(reg) cr4 & !(1u64 << 7), options(nomem, nostack));
    core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack));
    TLB_STATS.full_flushes.fetch_add(1, Ordering::Relaxed);
}

/// Invalide une plage de pages sur le CPU courant.
///
/// SAFETY: Chaque adresse dans [start..end) doit être une adresse canonique.
pub unsafe fn flush_range(start: VirtAddr, end: VirtAddr) {
    let mut addr = start.as_u64() & !(PAGE_SIZE as u64 - 1);
    let end_addr = (end.as_u64() + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
    while addr < end_addr {
        invlpg(VirtAddr::new(addr));
        addr += PAGE_SIZE as u64;
    }
    TLB_STATS.range_flushes.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// TLB SHOOTDOWN IPI
// ─────────────────────────────────────────────────────────────────────────────

/// Pending TLB shootdown — une opération à exécuter sur les CPUs cibles.
pub struct TlbShootdownRequest {
    pub flush_type: TlbFlushType,
    pub cpu_mask: u64,        // Bitmask des CPUs cibles (max 64 CPUs)
    pub completed: AtomicU64, // Bitmask des CPUs ayant terminé
}

/// File d'attente de TLB shootdowns globale.
pub struct TlbShootdownQueue {
    inner: Mutex<TlbShootdownInner>,
    pending: AtomicUsize,
}

struct TlbShootdownInner {
    requests: [TlbShootdownEntry; 8],
    head: usize,
    tail: usize,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
struct TlbShootdownEntry {
    active: bool,
    flush_type: TlbFlushType,
    cpu_mask: u64,
    completed: u64,
}

impl TlbShootdownEntry {
    const fn empty() -> Self {
        TlbShootdownEntry {
            active: false,
            flush_type: TlbFlushType::All,
            cpu_mask: 0,
            completed: 0,
        }
    }
}

impl TlbShootdownQueue {
    pub const fn new() -> Self {
        TlbShootdownQueue {
            inner: Mutex::new(TlbShootdownInner {
                requests: [TlbShootdownEntry::empty(); 8],
                head: 0,
                tail: 0,
            }),
            pending: AtomicUsize::new(0),
        }
    }

    /// Soumet une demande de TLB shootdown.
    /// Envoie un IPI aux CPUs cibles (via le vecteur IPI_TLB_SHOOTDOWN).
    ///
    /// SAFETY: `cpu_mask` doit être un masque valide des CPUs actifs.
    pub unsafe fn request(&self, flush_type: TlbFlushType, cpu_mask: u64) {
        {
            let mut inner = self.inner.lock();
            let tail = inner.tail;
            let next = (tail + 1) % 8;
            if next != inner.head {
                inner.requests[tail] = TlbShootdownEntry {
                    active: true,
                    flush_type,
                    cpu_mask,
                    completed: 0,
                };
                inner.tail = next;
                self.pending.fetch_add(1, Ordering::Release);
            }
        }
        // Envoyer l'IPI (le vecteur sera configuré par le sous-système APIC).
        // Pour ne pas créer de dépendance circulaire Couche 0, l'IPI est envoyé
        // via un pointeur de fonction enregistré au boot.
        Self::send_tlb_ipi(cpu_mask);
        TLB_STATS.ipi_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Handler exécuté sur chaque CPU cible en réception de l'IPI TLB.
    ///
    /// SAFETY: Doit être appelé depuis le handler d'interruption APIC.
    pub unsafe fn handle_remote(&self, cpu_id: u8) {
        if self.pending.load(Ordering::Acquire) == 0 {
            return;
        }
        let inner = self.inner.lock();
        for entry in &inner.requests {
            if !entry.active {
                continue;
            }
            if (entry.cpu_mask >> cpu_id) & 1 == 0 {
                continue;
            }
            match entry.flush_type {
                TlbFlushType::Single(addr) => flush_single(addr),
                TlbFlushType::Range { start, end } => flush_range(start, end),
                TlbFlushType::All => flush_all(),
                TlbFlushType::Global => flush_all_including_global(),
            }
        }
        // V-04 — Signaler que ce CPU a complété son flush (pour shootdown_sync).
        let seq = TLB_SHOOTDOWN_SEQ.load(Ordering::Acquire);
        if (cpu_id as usize) < 64 {
            TLB_SHOOTDOWN_ACK[cpu_id as usize].store(seq, Ordering::Release);
        }
    }

    unsafe fn send_tlb_ipi(cpu_mask: u64) {
        let fp = TLB_IPI_SENDER.load(Ordering::Acquire);
        if fp != 0 {
            let f: unsafe fn(u64) = core::mem::transmute(fp);
            f(cpu_mask);
        }
    }
}

static TLB_IPI_SENDER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// V-04 — COMPLETION TRACKING POUR SHOOTDOWN SYNCHRONE
// ─────────────────────────────────────────────────────────────────────────────

/// Numéro de séquence global de shootdown TLB (monotone croissant).
pub static TLB_SHOOTDOWN_SEQ: AtomicU64 = AtomicU64::new(0);

/// ACK par CPU — chaque CPU écrit la dernière séquence traitée.
pub static TLB_SHOOTDOWN_ACK: [AtomicU64; 64] = {
    const A: AtomicU64 = AtomicU64::new(0);
    [A; 64]
};

/// Enregistre la fonction d'envoi d'IPI TLB (fournie par le sous-système APIC).
///
/// SAFETY: `func` doit être une fonction valide prenant un cpu_mask en paramètre.
pub unsafe fn register_tlb_ipi_sender(func: unsafe fn(u64)) {
    TLB_IPI_SENDER.store(func as usize, Ordering::SeqCst);
}

/// File de TLB shootdown globale.
pub static TLB_QUEUE: TlbShootdownQueue = TlbShootdownQueue::new();

/// Performe un TLB shootdown asynchrone sur les CPUs donnés (fire-and-forget).
///
/// SAFETY: `cpu_mask` est un masque valide, interruptions non désactivées.
pub unsafe fn shootdown(flush_type: TlbFlushType, cpu_mask: u64) {
    TLB_QUEUE.request(flush_type, cpu_mask);
}

/// Performe un TLB shootdown SYNCHRONE sur tous les CPUs actifs.
///
/// Avance le numéro de séquence, envoie les IPIs, puis attend que chaque CPU
/// cible ait mis à jour son ACK (V-04 : TLB shootdown complété avant free_pages).
///
/// SAFETY: Ne pas appeler depuis un contexte IRQ. `cpu_count` doit refléter le
///         nombre réel de CPUs actifs (max 64).
pub unsafe fn shootdown_sync(flush_type: TlbFlushType, cpu_count: u32) {
    if cpu_count == 0 {
        return;
    }
    let n = cpu_count.min(64) as usize;
    let all_mask: u64 = if n >= 64 { !0u64 } else { (1u64 << n) - 1 };

    // Avancer la séquence — les CPUs devront ACK avec >= target_seq.
    let target_seq = TLB_SHOOTDOWN_SEQ.fetch_add(1, Ordering::SeqCst) + 1;

    // Envoyer les IPIs aux CPUs cibles.
    TLB_QUEUE.request(flush_type, all_mask);

    // Attendre que tous les CPUs cibles aient complété leur flush.
    for cpu_id in 0..n {
        loop {
            let ack = TLB_SHOOTDOWN_ACK[cpu_id].load(Ordering::Acquire);
            if ack >= target_seq {
                break;
            }
            core::hint::spin_loop();
        }
    }
}
