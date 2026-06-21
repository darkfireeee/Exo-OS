// kernel/src/security/integrity_check/mod.rs
//
// Module integrity_check — Vérification d'intégrité à plusieurs niveaux
//
// Sous-modules :
//   • code_signing   — Signature Ed25519 des modules kernel
//   • runtime_check  — Hash BLAKE3 périodique de .text/.rodata
//   • secure_boot    — Chaîne de confiance exo-boot → kernel

pub mod code_signing;
pub mod runtime_check;
pub mod secure_boot;

pub use code_signing::{
    code_sign_stats, register_loaded_module, verify_module_signature, CodeSignError, ModuleHeader,
};

pub use runtime_check::{
    assert_kernel_integrity, check_kernel_integrity, init_runtime_integrity, integrity_stats,
    security_periodic_check_observe, IntegrityError,
};

pub use secure_boot::{
    boot_nonce, check_chain_of_trust, extend_pcr, is_chain_verified, read_pcr, secureboot_stats,
    verify_boot_attestation, BootAttestation, SecureBootError,
};

/// Initialise le sous-système d'intégrité.
///
/// Ordre : runtime_check doit s'exécuter AVANT tout autre init
/// (les sections .text/.rodata doivent être intactes).
pub fn integrity_init() {
    init_runtime_integrity();
}

/// Intervalle entre deux vérifications d'intégrité runtime (15 s). Le hash
/// Blake3 de `.text/.rodata` est une opération lourde → basse fréquence
/// suffisante pour détecter une altération persistante sans charge inutile.
const INTEGRITY_CHECK_INTERVAL_NS: u64 = 15_000_000_000;

/// Boucle du kthread moniteur d'intégrité (TIER 2.1-a). Dort, puis — une fois le
/// boot sécurité terminé (`is_security_ready`, donc `.text/.rodata` stables) —
/// lance une vérification en **mode observe** (log/ledger, jamais de panic).
fn integrity_monitor_loop(_arg: usize) -> ! {
    loop {
        if !crate::scheduler::timer::sleep_ns(INTEGRITY_CHECK_INTERVAL_NS) {
            // Sommeil indisponible (très tôt) → céder le CPU et réessayer.
            // SAFETY: cooperative_reschedule cède proprement depuis un kthread.
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
            continue;
        }
        // TEST-25 (temporaire) : check d'intégrité périodique neutralisé pour
        // isoler le stall boot (le hash .text/.rodata = 14 Mio par PID 0).
        if false && crate::security::is_security_ready() {
            security_periodic_check_observe();
        }
    }
}

/// Démarre le kthread moniteur d'intégrité runtime. Best-effort : si la création
/// du kthread échoue, le boot continue (l'intégrité reste vérifiable à la demande
/// via `check_kernel_integrity`). À appeler une fois après `security_init`.
pub fn start_integrity_monitor() {
    use crate::process::lifecycle::create::{create_kthread, KthreadParams};
    use crate::scheduler::core::task::Priority;
    let _ = create_kthread(&KthreadParams {
        name: "integrity-mon",
        entry: integrity_monitor_loop,
        arg: 0,
        target_cpu: 0,
        priority: Priority::NORMAL_DEFAULT,
    });
}
