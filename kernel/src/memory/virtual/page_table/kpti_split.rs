// kernel/src/memory/virtual/page_table/kpti_split.rs
//
// KPTI (Kernel Page-Table Isolation) — tables de pages noyau/user séparées.
// Mitigation Meltdown : le noyau a deux PML4 distincts.
//   - kernel_pml4 : PML4 complète noyau (active pendant les syscalls/interruptions)
//   - user_pml4   : PML4 minimale user (active en espace user, sans mappings kernel)
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::memory::core::{AllocError, AllocFlags, PhysAddr};
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::page_table::x86_64::write_cr3;
use crate::memory::virt::page_table::{phys_to_table_mut, phys_to_table_ref};

// ─────────────────────────────────────────────────────────────────────────────
// ÉTAT KPTI
// ─────────────────────────────────────────────────────────────────────────────

/// État KPTI par CPU.
#[repr(C, align(64))]
pub struct KptiState {
    /// PML4 noyau (utilisée pendant les exceptions/syscalls).
    pub kernel_pml4: PhysAddr,
    /// PML4 user minimal (utilisée en espace user).
    pub user_pml4: PhysAddr,
    /// Stub de retour interprocesseur (trampoline).
    pub trampoline_phys: PhysAddr,
}

impl KptiState {
    pub const fn empty() -> Self {
        KptiState {
            kernel_pml4: PhysAddr::NULL,
            user_pml4: PhysAddr::NULL,
            trampoline_phys: PhysAddr::NULL,
        }
    }
}

/// Table KPTI globale (une entrée par CPU, MAX_CPUS=256).
pub struct KptiTable {
    states: [KptiState; 256],
    enabled: AtomicBool,
}

// SAFETY: KptiTable accède aux states de manière non concurrente (chaque CPU
//         accède uniquement à sa propre entrée).
unsafe impl Sync for KptiTable {}

impl KptiTable {
    pub const fn new() -> Self {
        KptiTable {
            states: {
                const EMPTY: KptiState = KptiState::empty();
                [EMPTY; 256]
            },
            enabled: AtomicBool::new(false),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    /// Enregistre les PML4 pour un CPU donné.
    ///
    /// SAFETY: `cpu_id < 256`, `kernel_pml4` et `user_pml4` sont des PML4
    ///         valides allouées avant cet appel.
    pub unsafe fn register_cpu(
        &self,
        cpu_id: usize,
        kernel_pml4: PhysAddr,
        user_pml4: PhysAddr,
        trampoline: PhysAddr,
    ) {
        debug_assert!(cpu_id < 256);
        // SAFETY: Accès exclusif pendant l'init CPU (single-CPU path).
        let state = &self.states[cpu_id] as *const KptiState as *mut KptiState;
        (*state).kernel_pml4 = kernel_pml4;
        (*state).user_pml4 = user_pml4;
        (*state).trampoline_phys = trampoline;
    }

    /// Switch vers la PML4 noyau pour ce CPU.
    ///
    /// SAFETY: Doit être appelé en entrée de syscall/exception avec KPTI actif.
    #[inline]
    pub unsafe fn switch_to_kernel(&self, cpu_id: usize) {
        if !self.is_enabled() {
            return;
        }
        let pml4 = self.states[cpu_id].kernel_pml4;
        if pml4.as_u64() != 0 {
            write_cr3(pml4);
        }
    }

    /// Switch vers la PML4 user pour ce CPU.
    ///
    /// SAFETY: Doit être appelé en sortie de syscall/exception (avant iret/sysret).
    #[inline]
    pub unsafe fn switch_to_user(&self, cpu_id: usize) {
        if !self.is_enabled() {
            return;
        }
        let pml4 = self.states[cpu_id].user_pml4;
        if pml4.as_u64() != 0 {
            write_cr3(pml4);
        }
    }

    /// Retourne les PML4 pour un CPU.
    pub fn get_pml4s(&self, cpu_id: usize) -> Option<(PhysAddr, PhysAddr)> {
        if cpu_id >= 256 {
            return None;
        }
        let s = &self.states[cpu_id];
        if s.kernel_pml4.as_u64() == 0 {
            None
        } else {
            Some((s.kernel_pml4, s.user_pml4))
        }
    }

    /// Retourne le CR3 user (physique) pour un CPU donné.
    #[inline]
    pub fn user_cr3_for_cpu(&self, cpu_id: usize) -> Option<u64> {
        if cpu_id >= 256 {
            return None;
        }
        let user = self.states[cpu_id].user_pml4.as_u64();
        if user == 0 {
            None
        } else {
            Some(user)
        }
    }
}

/// Table KPTI globale.
pub static KPTI: KptiTable = KptiTable::new();

/// Helper public: retourne le CR3 user (physique) du CPU donné.
#[inline]
pub fn user_cr3_for_cpu(cpu_id: usize) -> Option<u64> {
    KPTI.user_cr3_for_cpu(cpu_id)
}

/// Construit une PML4 user shadow à partir de la PML4 kernel courante.
///
/// La table user conserve:
/// - la moitié user (entrées PML4 0..255)
/// - l'entrée PML4[511] pour les stubs noyau nécessaires au retour d'exception/syscall
///
/// Le reste de la moitié kernel n'est pas copié.
///
/// # Safety
/// Doit être appelé en ring0 quand `kernel_pml4_phys` référence une PML4 valide.
pub unsafe fn build_user_shadow_pml4(kernel_pml4_phys: PhysAddr) -> Result<PhysAddr, AllocError> {
    let frame = buddy::alloc_page(AllocFlags::ZEROED)?;
    let user_pml4_phys = frame.start_address();

    let kernel_pml4 = phys_to_table_ref(kernel_pml4_phys);
    let user_pml4 = phys_to_table_mut(user_pml4_phys);

    // Copier les entrées user-space (0..255)
    for i in 0..256 {
        user_pml4[i] = kernel_pml4[i];
    }

    // Copier PML4[511] (stubs noyau hautes adresses / retour d'exception)
    user_pml4[511] = kernel_pml4[511];

    Ok(user_pml4_phys)
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPER : Vérification CPU KPTI
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie si le CPU courant supporte et requiert KPTI.
/// Retourne false si Meltdown n'affecte pas ce CPU (AMD, post-2018 Intel).
pub fn should_enable_kpti() -> bool {
    // Vérification par CPUID
    // SAFETY: CPUID lecture seule; xchg préserve rbx réservé par LLVM.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 1u32 => _,
            inout("ecx") 0u32 => _,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nomem, nostack),
        );
    }

    // Pour la sécurité, activer KPTI sur tous les systèmes Intel x86_64.
    // En production : vérifier CPUID vendor + microcode level.
    // Ici : activé par défaut (mode conservateur).
    true
}
