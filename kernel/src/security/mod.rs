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
//   └── audit/           Logger ring-buffer, règles d'audit, intégration syscall
//
// Ordre d'initialisation v6 (security_init) :
//   integrity_check → capability → access_control → zero_trust → crypto
//   → isolation → exploit_mitigations → audit
//
// RÈGLE SEC-INIT-01 : Aucun sous-système ne doit être utilisé avant security_init().
// RÈGLE SEC-INIT-02 : integrity_check::integrity_init() doit être le premier
//                     sous-système à s'exécuter après le boot (avant IRQs).
// ═══════════════════════════════════════════════════════════════════════════════

pub mod access_control;
pub mod capability;
pub mod zero_trust;
pub mod crypto;
pub mod isolation;
pub mod integrity_check;
pub mod exploit_mitigations;
pub mod audit;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — capability
// ─────────────────────────────────────────────────────────────────────────────

pub use capability::{
    CapToken,
    Rights,
    CapError,
    verify,
    revoke,
    delegate,
    init_capability_subsystem,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — access_control (v6)
// ─────────────────────────────────────────────────────────────────────────────

pub use access_control::{
    check_access,
    ObjectKind,
    AccessError,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — zero_trust
// ─────────────────────────────────────────────────────────────────────────────

pub use zero_trust::{
    SecurityLabel,
    SecurityContext,
    verify_access,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — crypto
// ─────────────────────────────────────────────────────────────────────────────

pub use crypto::{
    crypto_init,
    rng_fill,
    rng_u64,
    rng_u32,
    rng_is_ready,
    blake3_hash,
    blake3_mac,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — isolation
// ─────────────────────────────────────────────────────────────────────────────

pub use isolation::{
    SecurityDomain,
    DomainContext,
    SandboxPolicy,
    NamespaceSet,
    PledgeSet,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — integrity_check
// ─────────────────────────────────────────────────────────────────────────────

pub use integrity_check::{
    integrity_init,
    verify_module_signature,
    check_kernel_integrity,
    assert_kernel_integrity,
    check_chain_of_trust,
    is_chain_verified,
    ModuleHeader,
    CodeSignError,
    SecureBootError,
    IntegrityError,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — exploit_mitigations
// ─────────────────────────────────────────────────────────────────────────────

pub use exploit_mitigations::{
    mitigations_init,
    kaslr_offset,
    phys_to_virt,
    virt_to_phys,
    kaslr_is_ready,
    is_kernel_address,
    is_safe_kernel_ptr,
    install_canary,
    check_canary,
    remove_canary,
    cfg_register_target,
    cfg_register_range,
    cfg_lock,
    cfg_validate_indirect_call,
    cfg_assert_indirect_call,
    cet_is_supported,
    cet_status,
    safe_stack_new_thread,
    safe_stack_check,
    safe_stack_remove_thread,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — audit
// ─────────────────────────────────────────────────────────────────────────────

pub use audit::{
    audit_init,
    log_event,
    log_security_violation,
    flush_to_userspace,
    audit_syscall_entry,
    audit_syscall_exit,
    audit_capability_deny,
    audit_file_deny,
    AuditCategory,
    AuditOutcome,
    AuditVerdict,
    AuditRecord,
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
    // ── 1. Intégrité ──────────────────────────────────────────────────────────
    integrity_init();

    // ── 2. Capabilities ───────────────────────────────────────────────────────
    capability::init_capability_subsystem();

    // ── 3. Zero Trust ─────────────────────────────────────────────────────────
    // (pas de fonction d'init dédiée — la politique est lazy-initialized)

    // ── 4. Crypto ─────────────────────────────────────────────────────────────
    crypto_init();

    // ── 5. Isolation ──────────────────────────────────────────────────────────
    // (domaines/namespaces/sandbox/pledge s'initialisent via leurs statics)

    // ── 6. Atténuations d'exploitation ────────────────────────────────────────
    mitigations_init(kaslr_entropy, phys_base);

    // ── 7. Audit ──────────────────────────────────────────────────────────────
    audit_init();

    // ── 8. Access Control (v6) ────────────────────────────────────────────────
    access_control::init();
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
