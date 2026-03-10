// kernel/src/scheduler/smp/affinity.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Affinité CPU — gestion des masques de CPU autorisés pour chaque thread
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::AtomicU64;
use crate::scheduler::core::task::CpuId;
use super::topology::{MAX_CPUS, nr_cpus};

// ─────────────────────────────────────────────────────────────────────────────
// CpuMask — masque de CPUs (jusqu'à 256 CPUs sur 4 × u64)
// ─────────────────────────────────────────────────────────────────────────────

const MASK_WORDS: usize = 4;  // 4 × 64 = 256 bits

/// Masque de CPUs autorisés.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CpuMask {
    bits: [u64; MASK_WORDS],
}

impl CpuMask {
    /// Masque vide.
    pub const fn empty() -> Self {
        Self { bits: [0; MASK_WORDS] }
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
        let cpu = cpu.0 as usize;
        if cpu < MAX_CPUS { self.bits[cpu / 64] & (1u64 << (cpu % 64)) != 0 }
        else { false }
    }

    /// Retourne le premier CPU du masque, ou `None` si le masque est vide.
    pub fn first(&self) -> Option<CpuId> {
        for (word_idx, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                return Some(CpuId((word_idx * 64 + word.trailing_zeros() as usize) as u32));
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
        for i in 0..MASK_WORDS { out.bits[i] = self.bits[i] & other.bits[i]; }
        out
    }

    /// Retourne `true` si le masque est vide.
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification d'affinité
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne `true` si le thread peut tourner sur `cpu`.
pub fn cpu_allowed(affinity: u64, cpu: CpuId) -> bool {
    // Le champ `affinity` du TCB est un masque 64 bits (CPUs 0–63).
    if (cpu.0 as usize) < 64 {
        affinity & (1u64 << cpu.0) != 0
    } else {
        // Pour les CPUs > 63, toujours autorisé (compatibilité).
        true
    }
}

/// Construit le masque `u64` d'affinité (CPUs 0–63 uniquement).
pub fn affinity_mask_from_cpu_mask(mask: &CpuMask) -> u64 {
    mask.bits[0]
}

/// Vérifie qu'au moins un CPU actif est dans l'affinité du thread.
/// Retourne l'affinité inchangée si valide, ou un masque avec TOUS les CPUs
/// si le masque résultant est vide (protection anti-deadlock).
pub fn sanitize_affinity(affinity: u64) -> u64 {
    if affinity == 0 {
        // Masque invalide → autorise tous les CPUs en dessous de nr_cpus().
        let n = nr_cpus().min(64);
        if n == 64 { u64::MAX } else { (1u64 << n) - 1 }
    } else {
        affinity
    }
}

/// Métriques.
pub static AFFINITY_VIOLATIONS: AtomicU64 = AtomicU64::new(0);
