// kernel/src/memory/virtual/address_space/tlb.rs
//
// Gestion du TLB (Translation Lookaside Buffer) — invalidations locales et IPI.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::memory::core::{VirtAddr, MAX_CPUS, PAGE_SIZE};
use crate::memory::virt::page_table::x86_64::invlpg;

const TLB_MASK_BITS: usize = u64::BITS as usize;

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
    pub base_cpu: usize,      // CPU logique correspondant au bit 0 du masque
    pub window_mask: u64,     // Fenetre de 64 CPUs cibles
    pub completed: AtomicU64, // Bitmask des CPUs ayant termine
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
    base_cpu: usize,
    window_mask: u64,
    completed: u64,
}

impl TlbShootdownEntry {
    const fn empty() -> Self {
        TlbShootdownEntry {
            active: false,
            flush_type: TlbFlushType::All,
            base_cpu: 0,
            window_mask: 0,
            completed: 0,
        }
    }
}

#[inline]
fn coalesce_flush_type(a: TlbFlushType, b: TlbFlushType) -> TlbFlushType {
    if a == b {
        a
    } else {
        TlbFlushType::Global
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
    /// SAFETY: `window_mask` doit être un masque valide des CPUs actifs dans la
    /// fenetre `[base_cpu, base_cpu + 64)`.
    pub unsafe fn request(&self, flush_type: TlbFlushType, base_cpu: usize, window_mask: u64) {
        {
            let mut inner = self.inner.lock();
            let slot = &mut inner.requests[0];
            let can_merge = slot.active && slot.base_cpu == base_cpu;
            let merged_flush = if can_merge {
                coalesce_flush_type(slot.flush_type, flush_type)
            } else {
                flush_type
            };
            let merged_mask = if can_merge {
                slot.window_mask | window_mask
            } else {
                window_mask
            };

            *slot = TlbShootdownEntry {
                active: true,
                flush_type: merged_flush,
                base_cpu,
                window_mask: merged_mask,
                completed: 0,
            };
            inner.head = 0;
            inner.tail = 1;
            self.pending.store(1, Ordering::Release);
        }
        // Envoyer l'IPI (le vecteur sera configuré par le sous-système APIC).
        // Pour ne pas créer de dépendance circulaire Couche 0, l'IPI est envoyé
        // via un pointeur de fonction enregistré au boot.
        Self::send_tlb_ipi(base_cpu, window_mask);
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
            let logical_cpu = cpu_id as usize;
            if logical_cpu < entry.base_cpu || logical_cpu >= entry.base_cpu + TLB_MASK_BITS {
                continue;
            }
            let bit = logical_cpu - entry.base_cpu;
            if (entry.window_mask >> bit) & 1 == 0 {
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
        if (cpu_id as usize) < MAX_CPUS {
            TLB_SHOOTDOWN_ACK[cpu_id as usize].store(seq, Ordering::Release);
        }
    }

    unsafe fn send_tlb_ipi(base_cpu: usize, window_mask: u64) {
        let fp = TLB_IPI_SENDER.load(Ordering::Acquire);
        if fp != 0 {
            let f: unsafe fn(usize, u64) = core::mem::transmute(fp);
            f(base_cpu, window_mask);
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
pub static TLB_SHOOTDOWN_ACK: [AtomicU64; MAX_CPUS] = {
    const A: AtomicU64 = AtomicU64::new(0);
    [A; MAX_CPUS]
};

/// Enregistre la fonction d'envoi d'IPI TLB (fournie par le sous-système APIC).
///
/// SAFETY: `func` doit être une fonction valide prenant une fenetre de CPUs
/// `(base_cpu, cpu_mask)` en paramètre.
pub unsafe fn register_tlb_ipi_sender(func: unsafe fn(usize, u64)) {
    TLB_IPI_SENDER.store(func as usize, Ordering::SeqCst);
}

/// File de TLB shootdown globale.
pub static TLB_QUEUE: TlbShootdownQueue = TlbShootdownQueue::new();

/// Performe un TLB shootdown asynchrone sur les CPUs donnés (fire-and-forget).
///
/// SAFETY: `cpu_mask` est un masque valide, interruptions non désactivées.
pub unsafe fn shootdown(flush_type: TlbFlushType, cpu_mask: u64) {
    TLB_QUEUE.request(flush_type, 0, cpu_mask);
}

/// Performe un TLB shootdown SYNCHRONE sur tous les CPUs actifs.
///
/// Avance le numéro de séquence, envoie les IPIs, puis attend que chaque CPU
/// cible ait mis à jour son ACK (V-04 : TLB shootdown complété avant free_pages).
///
/// SAFETY: Ne pas appeler depuis un contexte IRQ. `cpu_count` doit refléter le
///         nombre réel de CPUs actifs (max [`MAX_CPUS`]).
pub unsafe fn shootdown_sync(flush_type: TlbFlushType, cpu_count: u32) {
    if cpu_count == 0 {
        return;
    }
    if cpu_count <= 1 || !crate::arch::x86_64::smp::smp_boot_complete() {
        match flush_type {
            TlbFlushType::Single(addr) => flush_single(addr),
            TlbFlushType::Range { start, end } => flush_range(start, end),
            TlbFlushType::All => flush_all(),
            TlbFlushType::Global => flush_all_including_global(),
        }
        return;
    }
    let n = (cpu_count as usize).min(MAX_CPUS);
    let current_cpu = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;

    // Avancer la séquence — les CPUs devront ACK avec >= target_seq.
    let target_seq = TLB_SHOOTDOWN_SEQ.fetch_add(1, Ordering::SeqCst) + 1;

    // TLB-01: l'émetteur purge d'abord son propre TLB.
    match flush_type {
        TlbFlushType::Single(addr) => flush_single(addr),
        TlbFlushType::Range { start, end } => flush_range(start, end),
        TlbFlushType::All => flush_all(),
        TlbFlushType::Global => flush_all_including_global(),
    }

    if current_cpu < n && current_cpu < MAX_CPUS {
        TLB_SHOOTDOWN_ACK[current_cpu].store(target_seq, Ordering::Release);
    }

    let mut base_cpu = 0usize;
    while base_cpu < n {
        let mut remote_mask = chunk_mask(n, base_cpu);
        if current_cpu >= base_cpu && current_cpu < base_cpu + TLB_MASK_BITS {
            remote_mask &= !(1u64 << (current_cpu - base_cpu));
        }

        // Envoyer les IPIs uniquement aux CPUs distants de cette fenetre.
        if remote_mask != 0 {
            TLB_QUEUE.request(flush_type, base_cpu, remote_mask);
        }

        // Attendre la fenetre courante avant de reutiliser le slot coalesce.
        wait_for_acks(base_cpu, n, target_seq);
        base_cpu += TLB_MASK_BITS;
    }
}

#[inline]
fn chunk_mask(cpu_count: usize, base_cpu: usize) -> u64 {
    if base_cpu >= cpu_count {
        return 0;
    }
    let width = (cpu_count - base_cpu).min(TLB_MASK_BITS);
    if width >= TLB_MASK_BITS {
        !0u64
    } else {
        (1u64 << width) - 1
    }
}

#[inline]
fn wait_for_acks(base_cpu: usize, cpu_count: usize, target_seq: u64) {
    let end = (base_cpu + TLB_MASK_BITS).min(cpu_count).min(MAX_CPUS);
    for cpu_id in base_cpu..end {
        loop {
            let ack = TLB_SHOOTDOWN_ACK[cpu_id].load(Ordering::Acquire);
            if ack >= target_seq {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{chunk_mask, TLB_MASK_BITS};

    #[test]
    fn chunk_mask_covers_first_full_window() {
        assert_eq!(chunk_mask(256, 0), !0u64);
    }

    #[test]
    fn chunk_mask_covers_tail_window_without_overflowing() {
        assert_eq!(chunk_mask(130, TLB_MASK_BITS * 2), 0b11);
    }

    #[test]
    fn chunk_mask_is_empty_after_cpu_count() {
        assert_eq!(chunk_mask(64, 64), 0);
    }
}
