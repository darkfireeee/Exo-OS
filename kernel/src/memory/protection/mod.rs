// kernel/src/memory/protection/mod.rs
//
// Module protection — NX / SMEP / SMAP / PKU.
//
// Ordre d'initialisation sur chaque CPU :
//   1. nx::init()     — EFER.NXE
//   2. smep::init()   — CR4.SMEP
//   3. smap::init()   — CR4.SMAP + CLAC initial
//   4. pku::init()    — CR4.PKE + PKRU initial
//
// Couche 0 : aucun import scheduler/process/ipc/fs.

pub mod nx;
pub mod pku;
pub mod smap;
pub mod smep;

// Re-exports NX
pub use nx::{
    enable_nx, is_nx_active, nx_enabled, nx_enforce_region, nx_handle_violation, nx_page_flags,
    nx_policy_for, NxPolicy, NxRegionRule, NxStats, NX_STATS, PAGE_TABLE_NX_BIT,
};

// Re-exports SMEP
pub use smep::{
    disable_smep, enable_smep, restore_smep, smep_active, smep_handle_violation, smep_supported,
    SmepGuard, SmepStats, CR4_SMEP_BIT, SMEP_STATS,
};

// Re-exports SMAP
pub use smap::{
    clac, copy_from_user, copy_to_user, disable_smap, enable_smap, smap_active,
    smap_handle_violation, smap_supported, stac, zero_user, SmapAccessGuard, SmapStats,
    CR4_SMAP_BIT, RFLAGS_AC_BIT, SMAP_STATS,
};

// Re-exports PKU
pub use pku::{
    enable_pku, pkru_ad_bit, pkru_wd_bit, pku_alloc_key, pku_allow_key, pku_deny_key, pku_free_key,
    pku_handle_violation, pku_readonly_key, pku_supported, pte_get_pkey, pte_set_pkey, rdpkru,
    wrpkru, PkuAccessGuard, PkuKeyDesc, PkuStats, CR4_PKE_BIT, CR4_PKS_BIT, PKU_DEFAULT_KEY,
    PKU_GUARD_KEY, PKU_KERNEL_HEAP_KEY, PKU_KEY_COUNT, PKU_MMIO_KEY, PKU_STATS, PTE_PKEY_MASK,
    PTE_PKEY_SHIFT,
};

/// Initialise tous les sous-systèmes de protection sur le CPU courant.
///
/// Doit être appelé sur BSP + chaque AP, après l'activation des page tables NX.
///
/// # Safety
/// CPL 0. Interruptions désactivées recommandées pour la séquence complète.
pub unsafe fn init() {
    nx::init();
    smep::init();
    smap::init();
    pku::init();
}
