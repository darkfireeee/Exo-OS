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
pub mod smep;
pub mod smap;
pub mod pku;

// Re-exports NX
pub use nx::{
    NxPolicy, NxRegionRule, NxStats, NX_STATS,
    PAGE_TABLE_NX_BIT,
    enable_nx, is_nx_active, nx_enabled,
    nx_policy_for, nx_page_flags, nx_enforce_region, nx_handle_violation,
};

// Re-exports SMEP
pub use smep::{
    CR4_SMEP_BIT, SmepStats, SMEP_STATS,
    smep_supported, enable_smep, disable_smep, restore_smep, smep_active,
    smep_handle_violation, SmepGuard,
};

// Re-exports SMAP
pub use smap::{
    CR4_SMAP_BIT, RFLAGS_AC_BIT, SmapStats, SMAP_STATS,
    smap_supported, enable_smap, disable_smap, smap_active,
    stac, clac, SmapAccessGuard,
    copy_from_user, copy_to_user, zero_user,
    smap_handle_violation,
};

// Re-exports PKU
pub use pku::{
    PKU_KEY_COUNT, PTE_PKEY_MASK, PTE_PKEY_SHIFT,
    CR4_PKE_BIT, CR4_PKS_BIT,
    PKU_DEFAULT_KEY, PKU_KERNEL_HEAP_KEY, PKU_GUARD_KEY, PKU_MMIO_KEY,
    pkru_ad_bit, pkru_wd_bit,
    PkuStats, PKU_STATS, PkuKeyDesc,
    pku_supported, enable_pku,
    pku_alloc_key, pku_free_key,
    pku_allow_key, pku_deny_key, pku_readonly_key,
    pte_set_pkey, pte_get_pkey,
    PkuAccessGuard, pku_handle_violation,
    rdpkru, wrpkru,
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
