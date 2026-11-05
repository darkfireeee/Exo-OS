// kernel/src/memory/protection/mod.rs
//
// Protection module — NX / SMEP / SMAP / UMIP / PKU.
//
// Initialization order on each CPU:
//   1. nx::init()     — EFER.NXE
//   2. smep::init()   — CR4.SMEP
//   3. smap::init()   — CR4.SMAP + initial CLAC
//   4. umip::init()   — CR4.UMIP
//   5. pku::init()    — CR4.PKE + initial PKRU
//
// Layer 0: no imports of scheduler/process/ipc/fs.

pub mod nx;
pub mod pku;
pub mod smap;
pub mod smep;
pub mod umip;

// NX re-exports
pub use nx::{
    enable_nx, is_nx_active, nx_enabled, nx_enforce_region, nx_handle_violation, nx_page_flags,
    nx_policy_for, NxPolicy, NxRegionRule, NxStats, NX_STATS, PAGE_TABLE_NX_BIT,
};

// SMEP re-exports
pub use smep::{
    disable_smep, enable_smep, restore_smep, smep_active, smep_handle_violation, smep_supported,
    SmepGuard, SmepStats, CR4_SMEP_BIT, SMEP_STATS,
};

// SMAP re-exports
pub use smap::{
    clac, copy_from_user, copy_to_user, disable_smap, enable_smap, smap_active,
    smap_handle_violation, smap_supported, stac, zero_user, SmapAccessGuard, SmapStats,
    CR4_SMAP_BIT, RFLAGS_AC_BIT, SMAP_STATS,
};

// UMIP re-exports
pub use umip::{enable_umip, umip_active, umip_supported, UmipStats, CR4_UMIP_BIT, UMIP_STATS};

// PKU re-exports
pub use pku::{
    enable_pku, pkru_ad_bit, pkru_wd_bit, pku_alloc_key, pku_allow_key, pku_deny_key, pku_free_key,
    pku_handle_violation, pku_readonly_key, pku_supported, pte_get_pkey, pte_set_pkey, rdpkru,
    wrpkru, PkuAccessGuard, PkuKeyDesc, PkuStats, CR4_PKE_BIT, CR4_PKS_BIT, PKU_DEFAULT_KEY,
    PKU_GUARD_KEY, PKU_KERNEL_HEAP_KEY, PKU_KEY_COUNT, PKU_MMIO_KEY, PKU_STATS, PTE_PKEY_MASK,
    PTE_PKEY_SHIFT,
};

/// Initializes all protection subsystems on current CPU.
///
/// Must be called on BSP + each AP, after activation of NX page tables.
///
/// # Safety
/// CPL 0. Interrupts disabled recommended for full sequence.
pub unsafe fn init() {
    nx::init();
    smep::init();
    smap::init();
    umip::init();
    pku::init();
}
