//! Security subsystem boot orchestration, periodic integrity checks, and
//! production hardening status reporting.
//!
//! Extracted from `security/mod.rs` so the module root stays a thin wiring facade.

use super::{
    access_control, assert_kernel_integrity, audit_init, blake3_hash, capability, crypto_init,
    exoargos, exocage, exokairos, exoledger, exonmi, exoseal, integrity_check, integrity_init,
    mitigations_init, rng_fill,
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

fn exoargos_context_switch_snapshot(tcb: &crate::scheduler::core::task::ThreadControlBlock) {
    let _ = exoargos::pmc_snapshot(tcb);
}

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
pub fn security_init(kaslr_entropy: u64, phys_base: u64) {
    #[inline(always)]
    fn probe(byte: u8) {
        crate::arch::x86_64::debug_e9_byte(byte);
    }

    probe(b'j');
    unsafe {
        exoseal::exoseal_boot_phase0();
    }
    probe(b'k');

    integrity_init();
    probe(b'l');

    capability::init_capability_subsystem();
    probe(b'm');

    // Zero Trust policy is lazy-initialized.
    probe(b'n');

    crypto_init();
    probe(b'o');

    // Isolation domains/namespaces/sandbox/pledge initialize via their statics.
    probe(b'p');

    mitigations_init(kaslr_entropy, phys_base);
    probe(b'q');

    audit_init();
    probe(b'r');

    access_control::init();
    probe(b's');

    exoledger::exo_ledger_init();
    probe(b't');

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

    unsafe {
        exoargos::exoargos_init();
    }
    crate::scheduler::core::switch::install_context_switch_out_hook(
        exoargos_context_switch_snapshot,
    );
    probe(b'v');

    exonmi::exonmi_init();
    probe(b'w');

    let current_tcb = crate::scheduler::core::switch::current_thread_raw();
    if !current_tcb.is_null() && exocage::is_cet_global_enabled() {
        let _ = unsafe { exocage::enable_cet_for_thread(&mut *current_tcb) };
    }
    probe(b'x');

    unsafe {
        exoseal::exoseal_boot_complete();
    }
    probe(b'y');
}

/// Performs periodic kernel integrity check.
#[inline]
pub fn security_periodic_check() {
    assert_kernel_integrity();
}

#[cfg(test)]
mod tests {
    use super::super::{is_security_ready, SECURITY_READY};
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

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

        let mut one_missing = all_on;
        one_missing.secure_boot_enforced = false;
        assert!(!one_missing.is_hardened());
    }

    #[test]
    fn security_ready_store_load_contract() {
        let local_flag = AtomicBool::new(false);
        assert!(!local_flag.load(Ordering::Acquire));
        local_flag.store(true, Ordering::Release);
        assert!(local_flag.load(Ordering::Acquire));
        assert!(local_flag.load(Ordering::Acquire));
    }

    #[test]
    fn is_security_ready_matches_atomic() {
        let raw = SECURITY_READY.load(Ordering::Acquire);
        assert_eq!(is_security_ready(), raw);
    }
}
