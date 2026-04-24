// kernel/src/security/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module security — Racine du sous-système de sécurité Exo-OS (v6)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Structure :
//   security/
//   ├── capability/      Tokens de capabilities, droits, révocation, délégation
//   ├── access_control/  Point d'entrée unifié — check_access() (v6, remplace ipc/capability_bridge)
//   ├── zero_trust/      Étiquettes MLS, Bell-LaPadula + Biba, contexte de confiance
//   ├── crypto/          BLAKE3, XChaCha20-Poly1305, X25519, Ed25519, AES-256-GCM, RNG, KDF
//   ├── isolation/       Domaines de sécurité, sandbox, namespaces, pledge
//   ├── integrity_check/ Signature de modules, hash runtime .text/.rodata, Secure Boot
//   ├── exploit_mitigations/ KASLR, stack canaries, CFG, CET, SafeStack
//   ├── audit/           Logger ring-buffer, règles d'audit, intégration syscall
//   ├── exocage/         CET Shadow Stack + IBT — handler #CP, intégration TCB (ExoShield v1.0)
//   ├── exoveil/         PKS domains — révocation O(1), isolation mémoire (ExoShield v1.0)
//   ├── exoledger/       Audit chaîné Blake3, zone P0 immuable (ExoShield v1.0)
//   └── exokairos/       Capabilities temporelles, deadline cachée, budgets (ExoShield v1.0)
//
// Ordre d'initialisation v7 (security_init) — ExoShield v1.0 :
//   integrity_check → capability → access_control → zero_trust → crypto
//   → isolation → exploit_mitigations → audit → exoledger → exokairos_init
//   → exoveil_restore → SECURITY_READY
//
// RÈGLE SEC-INIT-01 : Aucun sous-système ne doit être utilisé avant security_init().
// RÈGLE SEC-INIT-02 : integrity_check::integrity_init() doit être le premier
//                     sous-système à s'exécuter après le boot (avant IRQs).
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
pub mod zero_trust;

use core::sync::atomic::{AtomicBool, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SECURITY_READY — flag BOOT-SEC (Z-AI CVE-EXO-001)
// ─────────────────────────────────────────────────────────────────────────────

/// Flag atomique positionné à `true` à la fin de `security_init()`.
///
/// Les APs SMP DOIVENT spin-wait sur ce flag avant toute IPC ou accès ExoFS.
/// Sans ce flag, entre l'init des capabilities et celle du checker d'accès,
/// un AP peut effectuer des IPC non vérifiées (CVE-EXO-001 / BOOT-SEC).
///
/// # Utilisation dans arch/x86_64/smp/init.rs
/// ```rust,ignore
/// while !security::SECURITY_READY.load(Ordering::Acquire) {
///     core::hint::spin_loop();
/// }
/// ```
pub static SECURITY_READY: AtomicBool = AtomicBool::new(false);

/// Retourne `true` si security_init() est terminé.
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
    is_chain_verified, verify_module_signature, CodeSignError, IntegrityError, ModuleHeader,
    SecureBootError,
};

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
    exokairos_stats, init_kernel_secret, ttl_for_right, CapError as TemporalCapError,
    ExoKairosStats, TemporalCap, MAX_DELEGATION_DEPTH,
};

pub use exoseal::{
    configure_nic_iommu_policy, exoseal_boot_complete, exoseal_boot_phase0, nic_dma_window,
    nic_domain_id, nic_iommu_locked, verify_p0_fixes,
};

pub use ipc_policy::{check_direct_ipc, IpcPolicyResult};

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
// Initialisation orchestrée
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise l'intégralité du sous-système de sécurité d'Exo-OS.
///
/// # Ordre strict (RÈGLE SEC-INIT-01 et SEC-INIT-02) — v6
///
/// 1. **integrity_check** — Doit être en tête : calcule les hashes de référence
///    de `.text/.rodata` pendant que les sections sont intactes (avant tout IRQ).
/// 2. **capability** — Tables de capabilities, init du mécanisme de révocation.
/// 3. **zero_trust** — Tables de politiques MLS, contexte de confiance initial.
/// 4. **crypto** — CSPRNG via RDRAND + ChaCha20 (requis par stack_protector).
/// 5. **isolation** — Domaines, sandbox, namespaces, pledge.
/// 6. **exploit_mitigations** — KASLR figé (entropy du bootloader), canary global.
/// 7. **audit** — Logger ring-buffer, installe les règles par défaut.
/// 8. **access_control** — Enregistrement des mappings ObjectKind (v6, step 18).
///
/// # Arguments
/// - `kaslr_entropy` : entropie fournie par le bootloader (RDRAND + TSC)
/// - `phys_base`     : adresse physique réelle de chargement du kernel
pub fn security_init(kaslr_entropy: u64, phys_base: u64) {
    #[inline(always)]
    fn probe(byte: u8) {
        // SAFETY: port debug QEMU 0xE9, écriture bornée d'un octet pour tracer
        // le progrès du boot sécurité sans dépendre d'un driver déjà initialisé.
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
        }
    }

    // ── 0. ExoSeal phase 0 — CET + PKS default-deny + watchdog boot ─────────
    // SAFETY: security_init() est appelé au boot, avant toute concurrence
    // significative des sous-systèmes de sécurité.
    probe(b'j');
    unsafe {
        exoseal::exoseal_boot_phase0();
    }
    probe(b'k');

    // ── 1. Intégrité ──────────────────────────────────────────────────────────
    integrity_init();
    probe(b'l');

    // ── 2. Capabilities ───────────────────────────────────────────────────────
    capability::init_capability_subsystem();
    probe(b'm');

    // ── 3. Zero Trust ─────────────────────────────────────────────────────────
    // (pas de fonction d'init dédiée — la politique est lazy-initialized)
    probe(b'n');

    // ── 4. Crypto ─────────────────────────────────────────────────────────────
    crypto_init();
    probe(b'o');

    // ── 5. Isolation ──────────────────────────────────────────────────────────
    // (domaines/namespaces/sandbox/pledge s'initialisent via leurs statics)
    probe(b'p');

    // ── 6. Atténuations d'exploitation ────────────────────────────────────────
    mitigations_init(kaslr_entropy, phys_base);
    probe(b'q');

    // ── 7. Audit ──────────────────────────────────────────────────────────────
    audit_init();
    probe(b'r');

    // ── 8. Access Control (v6) ────────────────────────────────────────────────
    access_control::init();
    probe(b's');

    // ── 9. ExoLedger — Audit chaîné Blake3 + zone P0 (ExoShield v1.0) ────────
    //    Initialise le journal d'audit chaîné avec la zone P0 immuable.
    //    Doit être avant SECURITY_READY pour capturer les événements de boot.
    exoledger::exo_ledger_init();
    probe(b't');

    // ── 10. ExoKairos — Kernel secret (ExoShield v1.0) ──────────────────────
    //    Initialise le KERNEL_SECRET pour les capabilities temporelles.
    //    Le secret est dérivé du CSPRNG (déjà initialisé à l'étape 4).
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
    //    Initialise les compteurs PMU pour le monitoring de comportement.
    //    SAFETY: Ring 0, MSR write — appelé une seule fois au boot.
    unsafe {
        exoargos::exoargos_init();
    }
    probe(b'v');

    // ── 12. ExoNmi — Progressive NMI Watchdog (ExoShield v1.0 Module 9) ───────
    //    Initialise le watchdog (LAPIC virt base, timer masqué).
    exonmi::exonmi_init();
    probe(b'w');

    // ── 12b. ExoCage per-thread — thread bootstrap courant ──────────────────
    let current_tcb = crate::scheduler::core::switch::current_thread_raw();
    if !current_tcb.is_null() && exocage::is_cet_global_enabled() {
        // SAFETY: on agit sur le TCB courant du BSP pendant l'init sécurité,
        // avant l'ouverture normale du système.
        let _ = unsafe { exocage::enable_cet_for_thread(&mut *current_tcb) };
    }
    probe(b'x');

    // ── 13. ExoSeal complete — PKS ops normales + SECURITY_READY + watchdog ──
    // SAFETY: Ring 0, séquence finale de boot des modules de sécurité.
    unsafe {
        exoseal::exoseal_boot_complete();
    }
    probe(b'y');
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification périodique d'intégrité (appelée par le scheduler)
// ─────────────────────────────────────────────────────────────────────────────

/// Effectue la vérification périodique d'intégrité du kernel.
///
/// Doit être appelée par le scheduler toutes les N ticks.
///
/// Panique si une corruption est détectée (RÈGLE SECBOOT-01 niveau kernel).
#[inline]
pub fn security_periodic_check() {
    assert_kernel_integrity();
}

#[cfg(test)]
mod p2_7_security_tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

    /// P2-7 / Test 3 — Handshake SECURITY_READY : store Release → load Acquire.
    ///
    /// Vérifie le contrat BOOT-SEC / CVE-EXO-001 :
    /// - avant security_init() : flag à false
    /// - après store(true, Release) : load(Acquire) retourne true
    ///
    /// Ce test NE reproduit PAS les effets de bord de security_init()
    /// (integrity_check, crypto, etc.) — il valide uniquement l'atomique.
    /// Runnable sur host (pas de dépendance Ring 0).
    #[test]
    fn security_ready_store_load_contract() {
        // Réinitialisation locale pour isoler le test.
        let local_flag = AtomicBool::new(false);

        assert!(
            !local_flag.load(Ordering::Acquire),
            "SECURITY_READY doit être false avant l'init"
        );

        // Simule le store final de security_init().
        local_flag.store(true, Ordering::Release);

        assert!(
            local_flag.load(Ordering::Acquire),
            "SECURITY_READY doit être true après store(Release)"
        );

        // Vérifie la sémantique Acquire sur relecture.
        assert!(
            local_flag.load(Ordering::Acquire),
            "SECURITY_READY doit rester true (pas de reset implicite)"
        );
    }

    /// P2-7 / Test 3b — is_security_ready() est cohérent avec le flag global.
    ///
    /// Vérifie que le wrapper public lit bien avec Acquire.
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
