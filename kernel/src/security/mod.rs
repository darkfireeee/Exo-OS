// kernel/src/security/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Security module — Exo-OS security subsystem root (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Structure:
//   security/
//   ├── capability/      Capability tokens, rights, revocation, delegation
//   ├── access_control/  Unified entry point — check_access() (v6, replaces ipc/capability_bridge)
//   ├── zero_trust/      MLS labels, Bell-LaPadula + Biba, trust context
//   ├── crypto/          BLAKE3, XChaCha20-Poly1305, X25519, Ed25519, AES-256-GCM, RNG, KDF
//   ├── isolation/       Security domains, sandbox, namespaces, pledge
//   ├── integrity_check/ Module signing, runtime .text/.rodata hash, Secure Boot
//   ├── exploit_mitigations/ KASLR, stack canaries, CFG, CET, SafeStack
//   ├── audit/           Ring-buffer logger, audit rules, syscall integration
//   ├── exocage/         CET Shadow Stack + IBT — #CP handler, TCB integration (ExoShield v1.0)
//   ├── exoveil/         PKS domains — O(1) revocation, memory isolation (ExoShield v1.0)
//   ├── exoledger/       Blake3 chained audit, immutable P0 zone (ExoShield v1.0)
//   └── exokairos/       Temporal capabilities, hidden deadline, budgets (ExoShield v1.0)
//
// Initialization order v7 (security_init) — ExoShield v1.0:
//   integrity_check → capability → access_control → zero_trust → crypto
//   → isolation → exploit_mitigations → audit → exoledger → exokairos_init
//   → exoveil_restore → SECURITY_READY
//
// RULE SEC-INIT-01: No subsystem must be used prior to security_init().
// RULE SEC-INIT-02: integrity_check::integrity_init() must be the first
//                   subsystem to execute after boot (before IRQs).
// ═══════════════════════════════════════════════════════════════════════════════

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
pub mod shield_feed;
pub mod zero_trust;

use core::sync::atomic::{AtomicBool, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SECURITY_READY — BOOT-SEC flag (Z-AI CVE-EXO-001)
// ─────────────────────────────────────────────────────────────────────────────

/// Atomic flag set to `true` at the end of `security_init()`.
///
/// SMP APs MUST spin-wait on this flag before any IPC or ExoFS access.
/// Without this flag, between capability init and access checker init,
/// an AP can perform unverified IPCs (CVE-EXO-001 / BOOT-SEC).
///
/// # Usage in arch/x86_64/smp/init.rs
/// ```rust,ignore
/// while !security::SECURITY_READY.load(Ordering::Acquire) {
///     core::hint::spin_loop();
/// }
/// ```
pub static SECURITY_READY: AtomicBool = AtomicBool::new(false);

/// Returns `true` if security_init() has completed.
#[inline]
pub fn is_security_ready() -> bool {
    SECURITY_READY.load(Ordering::Acquire)
}

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — capability
// ─────────────────────────────────────────────────────────────────────────────

pub use capability::{
    alloc_namespace_id, can_delegate, create as cap_create, cross_namespace_verify, delegate,
    delegate_all, delegate_read_only, init_capability_subsystem, is_initialized, revoke,
    revoke_handle as cap_revoke_handle, revoke_token, token_stats, verify, verify_and_get_rights,
    verify_ipc_recv, verify_ipc_send, verify_read, verify_read_write, verify_typed, CapError,
    CapNamespace, CapObjectType, CapTable, CapTableSnapshot, CapToken, DelegationChain,
    DelegationEntry, KernelCapError, NamespaceId, ObjectId, Rights, TokenStats, CAP_TABLE_CAPACITY,
    CAP_TOKEN_WIRE_SIZE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — access_control (v6)
// ─────────────────────────────────────────────────────────────────────────────

pub use access_control::{check_access, AccessError, ObjectKind};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — zero_trust
// ─────────────────────────────────────────────────────────────────────────────

pub use zero_trust::{verify_access, SecurityContext, SecurityLabel};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — crypto
// ─────────────────────────────────────────────────────────────────────────────

pub use crypto::{blake3_hash, blake3_mac, crypto_init, rng_fill, rng_is_ready, rng_u32, rng_u64};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — isolation
// ─────────────────────────────────────────────────────────────────────────────

pub use isolation::{DomainContext, NamespaceSet, PledgeSet, SandboxPolicy, SecurityDomain};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — integrity_check
// ─────────────────────────────────────────────────────────────────────────────

pub use integrity_check::{
    assert_kernel_integrity, check_chain_of_trust, check_kernel_integrity, integrity_init,
    is_chain_verified, security_periodic_check_observe, start_integrity_monitor,
    verify_module_signature, CodeSignError, IntegrityError, ModuleHeader, SecureBootError,
};

// ─────────────────────────────────────────────────────────────────────────────
// FIX-F6 — Production hardening report
// ─────────────────────────────────────────────────────────────────────────────

/// State of critical protections expected in **production**. Distinguishes what
/// is *actually active* from what is disabled in dev, to prevent a production
/// build from being deployed without hardening (cf. AUDIT-100-PERCENT.md F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductionSecurityStatus {
    /// Feature `strict_exec_signatures` → exec rejects unsigned binaries.
    pub strict_exec_signatures: bool,
    /// Secure Boot enforcement active → mandatory chain of trust.
    pub secure_boot_enforced: bool,
    /// Embedded Kernel-A reference hashes (≠ dev ZERO_HASH).
    pub reference_hashes_embedded: bool,
}

impl ProductionSecurityStatus {
    /// True if ALL production protections are active.
    pub fn is_hardened(&self) -> bool {
        self.strict_exec_signatures && self.secure_boot_enforced && self.reference_hashes_embedded
    }
}

/// Computes production hardening status (pure function, testable).
pub fn production_security_status() -> ProductionSecurityStatus {
    ProductionSecurityStatus {
        strict_exec_signatures: cfg!(feature = "strict_exec_signatures"),
        secure_boot_enforced: integrity_check::secure_boot::enforcement_enabled(),
        reference_hashes_embedded: !crate::exophoenix::forge::kernel_a_hash_is_zero(),
    }
}

/// Emits a loud warning if the configuration is NOT hardened for
/// production. To be called once at the end of security init.
pub fn warn_if_not_production_hardened() {
    let st = production_security_status();
    if st.is_hardened() {
        return;
    }
    #[cfg(target_os = "none")]
    {
        use crate::arch::x86_64::terminal::debug_write;
        debug_write(b"[SECURITE] AVERTISSEMENT : build NON durci pour la production\n");
        if !st.strict_exec_signatures {
            debug_write(b"  - strict_exec_signatures DESACTIVE (binaires non signes acceptes)\n");
        }
        if !st.secure_boot_enforced {
            debug_write(b"  - Secure Boot enforcement DESACTIVE\n");
        }
        if !st.reference_hashes_embedded {
            debug_write(b"  - hashes Kernel-A non embarques (ZERO_HASH dev)\n");
        }
    }
}

#[cfg(test)]
mod production_status_tests {
    use super::*;

    #[test]
    fn status_reflects_strict_exec_feature_flag() {
        let st = production_security_status();
        assert_eq!(
            st.strict_exec_signatures,
            cfg!(feature = "strict_exec_signatures")
        );
    }

    #[test]
    fn hardened_requires_all_three_protections() {
        let all_on = ProductionSecurityStatus {
            strict_exec_signatures: true,
            secure_boot_enforced: true,
            reference_hashes_embedded: true,
        };
        assert!(all_on.is_hardened());

        let none_on = ProductionSecurityStatus {
            strict_exec_signatures: false,
            secure_boot_enforced: false,
            reference_hashes_embedded: false,
        };
        assert!(!none_on.is_hardened());

        // A single missing protection is enough to invalidate hardening.
        let mut one_missing = all_on;
        one_missing.secure_boot_enforced = false;
        assert!(!one_missing.is_hardened());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — exploit_mitigations
// ─────────────────────────────────────────────────────────────────────────────

pub use exploit_mitigations::{
    cet_is_supported, cet_status, cfg_assert_indirect_call, cfg_lock, cfg_register_range,
    cfg_register_target, cfg_validate_indirect_call, check_canary, install_canary,
    is_kernel_address, is_safe_kernel_ptr, kaslr_is_ready, kaslr_offset, mitigations_init,
    phys_to_virt, remove_canary, safe_stack_check, safe_stack_new_thread, safe_stack_remove_thread,
    virt_to_phys,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — audit
// ─────────────────────────────────────────────────────────────────────────────

pub use audit::{
    audit_capability_deny, audit_file_deny, audit_init, audit_syscall_entry, audit_syscall_exit,
    flush_to_userspace, log_event, log_security_violation, AuditCategory, AuditOutcome,
    AuditRecord, AuditVerdict,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — ExoShield v1.0 modules
// ─────────────────────────────────────────────────────────────────────────────

pub use exocage::{
    cet_thread_count, cp_handler, cp_violation_count, cpuid_cet_available, disable_cet_for_thread,
    enable_cet_for_thread, exocage_global_enable, exocage_stats, is_cet_global_enabled,
    is_ibt_global_enabled, validate_thread_cet, ExoCageError, ExoCageStats, CET_FLAG_ENABLED,
    CET_FLAG_IBT, CET_FLAG_TOKEN_VALID,
};

pub use exoveil::{
    current_pkrs, exoveil_init, exoveil_initialized, exoveil_revoke_all_on_handoff, exoveil_stats,
    is_domain_revoked, pks_available, pks_restore_for_normal_ops, restore_domain,
    restore_domain_with_permission, restore_pkrs_from_tcb, revoke_count, revoke_domain,
    save_pkrs_to_tcb, ExoVeilStats, PksDomain, PksPermission,
};

pub use exoledger::{
    current_seq, exo_ledger_append, exo_ledger_append_p0, exo_ledger_init, exoledger_stats,
    p0_used, read_p0_entry, read_ring_entry, total_written, verify_p0_integrity,
    verify_ring_integrity, ActionTag, AuditHeader, ExoLedgerStats, LedgerEntry,
    LedgerIntegrityError, P0_ZONE_ENTRIES, SSR_LOG_AUDIT_OFFSET, SSR_LOG_AUDIT_SIZE,
};

pub use exokairos::{
    exokairos_stats, init_kernel_secret, register_ttl_for_cap, ttl_for_right,
    CapError as TemporalCapError, ExoKairosStats, TemporalCap, MAX_DELEGATION_DEPTH,
};

pub use exoseal::{
    configure_nic_iommu_policy, exoseal_boot_complete, exoseal_boot_phase0, nic_dma_window,
    nic_domain_id, nic_iommu_locked, verify_p0_fixes,
};

pub use ipc_policy::{
    can_inject_src_pid, check_direct_ipc, register_service, register_service_class,
    service_class_of, unregister_service, IpcPolicyResult, ServiceClass,
};

fn exoargos_context_switch_snapshot(tcb: &crate::scheduler::core::task::ThreadControlBlock) {
    let _ = exoargos::pmc_snapshot(tcb);
}

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — ExoShield v1.0 Module 9 : ExoNmi
// ─────────────────────────────────────────────────────────────────────────────

pub use exonmi::{
    arm_watchdog, configured_timeout_ms, current_strikes, exonmi_init, exonmi_stats, is_armed,
    ping, tick, ExoNmiStats,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — ExoShield v1.0 Module 8 : ExoArgos
// ─────────────────────────────────────────────────────────────────────────────

pub use exoargos::{
    baseline_established, check_anomaly, compute_discordance, exoargos_init, exoargos_stats,
    get_baseline, init_pmu, pmc_snapshot, update_baseline, ExoArgosStats, PmcSnapshot,
    DECEPTION_THRESHOLD,
};

// ─────────────────────────────────────────────────────────────────────────────
// Orchestrated initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the entirety of the Exo-OS security subsystem.
///
/// # Strict order (RULE SEC-INIT-01 and SEC-INIT-02) — v6
///
/// 1. **integrity_check** — Must lead: calculates reference hashes of
///    `.text/.rodata` while sections are intact (prior to any IRQ).
/// 2. **capability** — Capability tables, init of revocation mechanism.
/// 3. **zero_trust** — MLS policy tables, initial trust context.
/// 4. **crypto** — CSPRNG via RDRAND + ChaCha20 (required by stack_protector).
/// 5. **isolation** — Domains, sandbox, namespaces, pledge.
/// 6. **exploit_mitigations** — Frozen KASLR (bootloader entropy), global canary.
/// 7. **audit** — Ring-buffer logger, installs default rules.
/// 8. **access_control** — ObjectKind mapping registration (v6, step 18).
///
/// # Arguments
/// - `kaslr_entropy`: entropy provided by bootloader (RDRAND + TSC)
/// - `phys_base`: actual physical load base address of kernel
pub fn security_init(kaslr_entropy: u64, phys_base: u64) {
    #[inline(always)]
    fn probe(byte: u8) {
        // SAFETY: QEMU debug port 0xE9, bounded single-byte write to trace
        // security boot progress without depending on an initialized driver.
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
        }
    }

    // ── 0. ExoSeal phase 0 — CET + PKS default-deny + boot watchdog ─────────
    // SAFETY: security_init() is called at boot, prior to any significant
    // concurrency across security subsystems.
    probe(b'j');
    unsafe {
        exoseal::exoseal_boot_phase0();
    }
    probe(b'k');

    // ── 1. Integrity ──────────────────────────────────────────────────────────
    integrity_init();
    probe(b'l');

    // ── 2. Capabilities ───────────────────────────────────────────────────────
    capability::init_capability_subsystem();
    probe(b'm');

    // ── 3. Zero Trust ─────────────────────────────────────────────────────────
    // (no dedicated init function — policy is lazy-initialized)
    probe(b'n');

    // ── 4. Crypto ─────────────────────────────────────────────────────────────
    crypto_init();
    probe(b'o');

    // ── 5. Isolation ──────────────────────────────────────────────────────────
    // (domains/namespaces/sandbox/pledge initialize via their statics)
    probe(b'p');

    // ── 6. Exploit mitigations ────────────────────────────────────────
    mitigations_init(kaslr_entropy, phys_base);
    probe(b'q');

    // ── 7. Audit ──────────────────────────────────────────────────────────────
    audit_init();
    probe(b'r');

    // ── 8. Access Control (v6) ────────────────────────────────────────────────
    access_control::init();
    probe(b's');

    // ── 9. ExoLedger — Blake3 chained audit + P0 zone (ExoShield v1.0) ────────
    //    Initializes chained audit log with immutable P0 zone.
    //    Must precede SECURITY_READY to capture boot events.
    exoledger::exo_ledger_init();
    probe(b't');

    // ── 10. ExoKairos — Kernel secret (ExoShield v1.0) ──────────────────────
    //    Initializes KERNEL_SECRET for temporal capabilities.
    //    Secret is derived from CSPRNG (already initialized in step 4).
    {
        let mut secret = [0u8; 32];
        if rng_fill(&mut secret).is_err() {
            let mut fallback_material = [0u8; 32];
            let tsc = crate::arch::x86_64::cpu::tsc::read_tsc();
            fallback_material[0..8].copy_from_slice(&kaslr_entropy.to_le_bytes());
            fallback_material[8..16].copy_from_slice(&phys_base.to_le_bytes());
            fallback_material[16..24].copy_from_slice(&tsc.to_le_bytes());
            fallback_material[24..32].copy_from_slice(
                &(kaslr_entropy.rotate_left(17) ^ phys_base.rotate_left(9) ^ tsc).to_le_bytes(),
            );
            secret = blake3_hash(&fallback_material);
        }
        exokairos::init_kernel_secret(&secret);
    }
    probe(b'u');

    // ── 11. ExoArgos — PMC Monitoring (ExoShield v1.0 Module 8) ──────────────
    //    Initializes PMU counters for behavioral monitoring.
    //    SAFETY: Ring 0, MSR write — called only once at boot.
    unsafe {
        exoargos::exoargos_init();
    }
    crate::scheduler::core::switch::install_context_switch_out_hook(
        exoargos_context_switch_snapshot,
    );
    probe(b'v');

    // ── 12. ExoNmi — Progressive NMI Watchdog (ExoShield v1.0 Module 9) ───────
    //    Initializes watchdog (LAPIC virt base, masked timer).
    exonmi::exonmi_init();
    probe(b'w');

    // ── 12b. ExoCage per-thread — current bootstrap thread ──────────────────
    let current_tcb = crate::scheduler::core::switch::current_thread_raw();
    if !current_tcb.is_null() && exocage::is_cet_global_enabled() {
        // SAFETY: operating on current BSP TCB during security init,
        // prior to normal system operation.
        let _ = unsafe { exocage::enable_cet_for_thread(&mut *current_tcb) };
    }
    probe(b'x');

    // ── 13. ExoSeal complete — PKS normal ops + SECURITY_READY + watchdog ──
    // SAFETY: Ring 0, final boot sequence of security modules.
    unsafe {
        exoseal::exoseal_boot_complete();
    }
    probe(b'y');
}

// ─────────────────────────────────────────────────────────────────────────────
// Periodic integrity check (called by scheduler)
// ─────────────────────────────────────────────────────────────────────────────

/// Performs periodic kernel integrity check.
///
/// Must be called by the scheduler every N ticks.
///
/// Panics if corruption is detected (RULE SECBOOT-01 kernel level).
#[inline]
pub fn security_periodic_check() {
    assert_kernel_integrity();
}

#[cfg(test)]
mod p2_7_security_tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

    /// P2-7 / Test 3 — SECURITY_READY handshake: store Release → load Acquire.
    ///
    /// Verifies BOOT-SEC / CVE-EXO-001 contract:
    /// - prior to security_init(): flag is false
    /// - after store(true, Release): load(Acquire) returns true
    ///
    /// This test DOES NOT reproduce side effects of security_init()
    /// (integrity_check, crypto, etc.) — it validates solely the atomic.
    /// Runnable on host (no Ring 0 dependency).
    #[test]
    fn security_ready_store_load_contract() {
        // Local reinitialization to isolate test.
        let local_flag = AtomicBool::new(false);

        assert!(
            !local_flag.load(Ordering::Acquire),
            "SECURITY_READY doit être false avant l'init"
        );

        // Simulates final store of security_init().
        local_flag.store(true, Ordering::Release);

        assert!(
            local_flag.load(Ordering::Acquire),
            "SECURITY_READY doit être true après store(Release)"
        );

        // Verifies Acquire semantics on reload.
        assert!(
            local_flag.load(Ordering::Acquire),
            "SECURITY_READY doit rester true (pas de reset implicite)"
        );
    }

    /// P2-7 / Test 3b — is_security_ready() is consistent with global flag.
    ///
    /// Verifies that public wrapper reads with Acquire.
    #[test]
    fn is_security_ready_matches_atomic() {
        let raw = SECURITY_READY.load(Ordering::Acquire);
        assert_eq!(
            is_security_ready(),
            raw,
            "is_security_ready() doit être cohérent avec SECURITY_READY.load(Acquire)"
        );
    }
}
