//! Security module — Exo-OS security subsystem root (v6).
//!
//! RULE SEC-INIT-01: No subsystem must be used prior to `security_init()`.
//! RULE SEC-INIT-02: `integrity_check::integrity_init()` runs first before IRQs.

pub mod access_control;
pub mod audit;
pub mod capability;
pub mod crypto;
pub mod exoargos;
pub mod exocage;
pub mod exokairos;
pub mod exoledger;
pub mod exonmi;
pub mod exoseal;
pub mod exoveil;
pub mod exploit_mitigations;
pub mod integrity_check;
pub mod ipc_policy;
pub mod isolation;
mod lifecycle;
pub mod shield_feed;
pub mod zero_trust;

use core::sync::atomic::{AtomicBool, Ordering};

/// Atomic flag set to `true` at the end of `security_init()` (BOOT-SEC / CVE-EXO-001).
pub static SECURITY_READY: AtomicBool = AtomicBool::new(false);

/// Returns `true` if `security_init()` has completed.
#[inline]
pub fn is_security_ready() -> bool {
    SECURITY_READY.load(Ordering::Acquire)
}

pub use access_control::{check_access, AccessError, ObjectKind};
pub use audit::{
    audit_capability_deny, audit_file_deny, audit_init, audit_syscall_entry, audit_syscall_exit,
    flush_to_userspace, log_event, log_security_violation, AuditCategory, AuditOutcome,
    AuditRecord, AuditVerdict,
};
pub use capability::{
    alloc_namespace_id, can_delegate, create as cap_create, cross_namespace_verify, delegate,
    delegate_all, delegate_read_only, init_capability_subsystem, is_initialized, revoke,
    revoke_handle as cap_revoke_handle, revoke_token, token_stats, verify, verify_and_get_rights,
    verify_ipc_recv, verify_ipc_send, verify_read, verify_read_write, verify_typed, CapError,
    CapNamespace, CapObjectType, CapTable, CapTableSnapshot, CapToken, DelegationChain,
    DelegationEntry, KernelCapError, NamespaceId, ObjectId, Rights, TokenStats, CAP_TABLE_CAPACITY,
    CAP_TOKEN_WIRE_SIZE,
};
pub use crypto::{blake3_hash, blake3_mac, crypto_init, rng_fill, rng_is_ready, rng_u32, rng_u64};
pub use exoargos::{
    baseline_established, check_anomaly, compute_discordance, exoargos_init, exoargos_stats,
    get_baseline, init_pmu, pmc_snapshot, update_baseline, ExoArgosStats, PmcSnapshot,
    DECEPTION_THRESHOLD,
};
pub use exocage::{
    cet_thread_count, cp_handler, cp_violation_count, cpuid_cet_available, disable_cet_for_thread,
    enable_cet_for_thread, exocage_global_enable, exocage_stats, is_cet_global_enabled,
    is_ibt_global_enabled, validate_thread_cet, ExoCageError, ExoCageStats, CET_FLAG_ENABLED,
    CET_FLAG_IBT, CET_FLAG_TOKEN_VALID,
};
pub use exokairos::{
    exokairos_stats, init_kernel_secret, register_ttl_for_cap, ttl_for_right,
    CapError as TemporalCapError, ExoKairosStats, TemporalCap, MAX_DELEGATION_DEPTH,
};
pub use exoledger::{
    current_seq, exo_ledger_append, exo_ledger_append_p0, exo_ledger_init, exoledger_stats,
    p0_used, read_p0_entry, read_ring_entry, total_written, verify_p0_integrity,
    verify_ring_integrity, ActionTag, AuditHeader, ExoLedgerStats, LedgerEntry,
    LedgerIntegrityError, P0_ZONE_ENTRIES, SSR_LOG_AUDIT_OFFSET, SSR_LOG_AUDIT_SIZE,
};
pub use exonmi::{
    arm_watchdog, configured_timeout_ms, current_strikes, exonmi_init, exonmi_stats, is_armed,
    ping, tick, ExoNmiStats,
};
pub use exoseal::{
    configure_nic_iommu_policy, exoseal_boot_complete, exoseal_boot_phase0, nic_dma_window,
    nic_domain_id, nic_iommu_locked, verify_p0_fixes,
};
pub use exoveil::{
    current_pkrs, exoveil_init, exoveil_initialized, exoveil_revoke_all_on_handoff, exoveil_stats,
    is_domain_revoked, pks_available, pks_restore_for_normal_ops, restore_domain,
    restore_domain_with_permission, restore_pkrs_from_tcb, revoke_count, revoke_domain,
    save_pkrs_to_tcb, ExoVeilStats, PksDomain, PksPermission,
};
pub use exploit_mitigations::{
    cet_is_supported, cet_status, cfg_assert_indirect_call, cfg_lock, cfg_register_range,
    cfg_register_target, cfg_validate_indirect_call, check_canary, install_canary,
    is_kernel_address, is_safe_kernel_ptr, kaslr_is_ready, kaslr_offset, mitigations_init,
    phys_to_virt, remove_canary, safe_stack_check, safe_stack_new_thread, safe_stack_remove_thread,
    virt_to_phys,
};
pub use integrity_check::{
    assert_kernel_integrity, check_chain_of_trust, check_kernel_integrity, integrity_init,
    is_chain_verified, security_periodic_check_observe, start_integrity_monitor,
    verify_module_signature, CodeSignError, IntegrityError, ModuleHeader, SecureBootError,
};
pub use ipc_policy::{
    can_inject_src_pid, check_direct_ipc, register_service, register_service_class,
    service_class_of, unregister_service, IpcPolicyResult, ServiceClass,
};
pub use isolation::{DomainContext, NamespaceSet, PledgeSet, SandboxPolicy, SecurityDomain};
pub use lifecycle::{
    production_security_status, security_init, security_periodic_check,
    warn_if_not_production_hardened, ProductionSecurityStatus,
};
pub use zero_trust::{verify_access, SecurityContext, SecurityLabel};
