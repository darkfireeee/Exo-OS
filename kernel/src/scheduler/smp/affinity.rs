// kernel/src/scheduler/smp/affinity.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Affinité CPU — gestion des masques de CPU autorisés pour chaque thread
// ═══════════════════════════════════════════════════════════════════════════════

use super::topology::{nr_cpus, MAX_CPUS};
use crate::scheduler::core::task::CpuId;
use core::sync::atomic::AtomicU64;

// ─────────────────────────────────────────────────────────────────────────────
// CpuMask — masque de CPUs (jusqu'à 256 CPUs sur 4 × u64)
// ─────────────────────────────────────────────────────────────────────────────

const MASK_WORDS: usize = 4; // 4 × 64 = 256 bits

/// Masque de CPUs autorisés.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuSet {
    pub(crate) bits: [u64; MASK_WORDS],
}

impl CpuSet {
    pub const EMPTY: Self = Self {
        bits: [0; MASK_WORDS],
    };
    pub const ALL: Self = Self {
        bits: [u64::MAX; MASK_WORDS],
    };

    /// Masque vide.
    pub const fn empty() -> Self {
        Self::EMPTY
    }

    #[inline(always)]
    pub const fn new(bits: [u64; MASK_WORDS]) -> Self {
        Self { bits }
    }

    /// Masque avec tous les CPUs actifs (jusqu'à `nr_cpus()`).
    pub fn full() -> Self {
        let mut m = Self::empty();
        for cpu in 0..nr_cpus().min(MAX_CPUS) {
            m.set(CpuId(cpu as u32));
        }
        m
    }

    /// Active le bit du CPU `cpu`.
    pub fn set(&mut self, cpu: CpuId) {
        let cpu = cpu.0 as usize;
        if cpu < MAX_CPUS {
            self.bits[cpu / 64] |= 1u64 << (cpu % 64);
        }
    }

    /// Désactive le bit du CPU `cpu`.
    pub fn clear(&mut self, cpu: CpuId) {
        let cpu = cpu.0 as usize;
        if cpu < MAX_CPUS {
            self.bits[cpu / 64] &= !(1u64 << (cpu % 64));
        }
    }

    /// Retourne `true` si le CPU `cpu` est dans le masque.
    pub fn test(&self, cpu: CpuId) -> bool {
        self.contains(cpu)
    }

    #[inline(always)]
    pub fn contains(&self, cpu: CpuId) -> bool {
        let cpu = cpu.0 as usize;
        if cpu < MAX_CPUS {
            self.bits[cpu / 64] & (1u64 << (cpu % 64)) != 0
        } else {
            false
        }
    }

    /// Retourne le premier CPU du masque, ou `None` si le masque est vide.
    pub fn first(&self) -> Option<CpuId> {
        self.first_cpu()
    }

    pub fn first_cpu(&self) -> Option<CpuId> {
        for (word_idx, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                return Some(CpuId(
                    (word_idx * 64 + word.trailing_zeros() as usize) as u32,
                ));
            }
        }
        None
    }

    /// Nombre de CPUs actifs dans le masque.
    pub fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Intersection (ET logique) de deux masques.
    pub fn and(&self, other: &Self) -> Self {
        let mut out = Self::empty();
        for i in 0..MASK_WORDS {
            out.bits[i] = self.bits[i] & other.bits[i];
        }
        out
    }

    /// Retourne `true` si le masque est vide.
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }

    /// Retourne un masque avec un seul CPU autorisé.
    pub fn single(cpu: CpuId) -> Self {
        let mut out = Self::EMPTY;
        out.set(cpu);
        out
    }
}

pub type CpuMask = CpuSet;

const _: () = assert!(
    core::mem::size_of::<CpuSet>() == 32,
    "CpuSet doit faire 32 bytes"
);

// ─────────────────────────────────────────────────────────────────────────────
// Vérification d'affinité
// ─────────────────────────────────────────────────────────────────────────────

/// Alias de compatibilité pour les call-sites migrés progressivement.
pub fn cpu_allowed(affinity: &CpuSet, cpu: CpuId) -> bool {
    affinity.contains(cpu)
}

/// Retourne un masque d'affinité stable depuis un CpuMask.
pub fn affinity_mask_from_cpu_mask(mask: &CpuMask) -> CpuSet {
    *mask
}

/// Vérifie qu'au moins un CPU actif est dans l'affinité du thread.
/// Retourne l'affinité inchangée si valide, ou un masque avec TOUS les CPUs
/// si le masque résultant est vide (protection anti-deadlock).
pub fn sanitize_affinity(affinity: CpuSet) -> CpuSet {
    if affinity.is_empty() {
        CpuSet::full()
    } else {
        affinity
    }
}

/// Métriques.
pub static AFFINITY_VIOLATIONS: AtomicU64 = AtomicU64::new(0);
