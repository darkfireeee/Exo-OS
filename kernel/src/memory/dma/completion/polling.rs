// kernel/src/memory/dma/completion/polling.rs
//
// Polling haute fréquence pour la complétion DMA.
//
// Utilisé quand un thread kernel doit attendre la fin d'un transfert DMA
// sans passer par le mécanisme de wakeup (contextes sans scheduler actif :
// init, early_boot, drivers embarqués).
//
// Deux modes :
//   - `spin_poll` : spin actif jusqu'à complétion ou timeout (TSC-based).
//   - `yield_poll` : même chose mais avec `pause` (REPNE) entre les lectures.
//
// COUCHE 0 — aucune dépendance externe. Pas de scheduler::sleep().

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::dma::core::types::{DmaTransactionId, DmaError};
use crate::memory::dma::completion::handler::DMA_COMPLETION;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Timeout par défaut en cycles TSC (~10 ms sur 3 GHz).
pub const DEFAULT_POLL_TIMEOUT_CYCLES: u64 = 30_000_000;

/// Timeout long pour les transferts DMA de gros volumes (~1 s sur 3 GHz).
pub const LONG_POLL_TIMEOUT_CYCLES: u64 = 3_000_000_000;

/// Nombre de lectures TSC sans `pause` avant d'insérer un `pause`.
const SPIN_BATCH_SIZE: u32 = 64;

// ─────────────────────────────────────────────────────────────────────────────
// RÉSULTAT DU POLLING
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une opération de polling.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PollResult {
    /// Transfert terminé avec succès. Octets transférés.
    Done(usize),
    /// Transfert terminé avec erreur.
    Error(DmaError),
    /// Timeout expiré sans complétion.
    Timeout,
    /// La transaction n'est pas dans la table de complétion.
    NotFound,
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES DE POLLING
// ─────────────────────────────────────────────────────────────────────────────

pub struct PollStats {
    /// Polls ayant réussi (Done).
    pub successes:    AtomicU64,
    /// Polls ayant échoué (Error).
    pub errors:       AtomicU64,
    /// Polls ayant expiré.
    pub timeouts:     AtomicU64,
    /// Cycles TSC totaux consommés par les polls réussis.
    pub cycles_total: AtomicU64,
    /// Cycles TSC max sur un poll réussi (worst-case latency).
    pub cycles_max:   AtomicU64,
}

impl PollStats {
    const fn new() -> Self {
        PollStats {
            successes:    AtomicU64::new(0),
            errors:       AtomicU64::new(0),
            timeouts:     AtomicU64::new(0),
            cycles_total: AtomicU64::new(0),
            cycles_max:   AtomicU64::new(0),
        }
    }
}

pub static POLL_STATS: PollStats = PollStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// MOTEUR DE POLLING
// ─────────────────────────────────────────────────────────────────────────────

/// Polling DMA en spin actif.
///
/// Tourne jusqu'à ce que :
///   - La transaction soit marquée `Done`/`Error` dans `DMA_COMPLETION`,
///   - Ou que `timeout_cycles` cycles TSC se soient écoulés.
///
/// # Paramètres
/// - `txn_id` : transaction à attendre.
/// - `timeout_cycles` : durée max en cycles TSC (0 = pas de timeout).
///
/// # Contexte
/// Peut être appelé avec interruptions désactivées.
/// Ne doit PAS être appelé depuis un contexte d'interruption (récursion).
pub fn spin_poll(txn_id: DmaTransactionId, timeout_cycles: u64) -> PollResult {
    let start = read_tsc();
    let mut batch = 0u32;

    loop {
        // Interroger l'état dans la completion table.
        match DMA_COMPLETION.query(txn_id) {
            Some(Ok(bytes)) => {
                let elapsed = read_tsc().wrapping_sub(start);
                POLL_STATS.successes.fetch_add(1, Ordering::Relaxed);
                POLL_STATS.cycles_total.fetch_add(elapsed, Ordering::Relaxed);
                // Mise à jour du max sans verrou (approximation acceptable).
                let old_max = POLL_STATS.cycles_max.load(Ordering::Relaxed);
                if elapsed > old_max {
                    POLL_STATS.cycles_max.store(elapsed, Ordering::Relaxed);
                }
                return PollResult::Done(bytes);
            }
            Some(Err(err)) => {
                POLL_STATS.errors.fetch_add(1, Ordering::Relaxed);
                return PollResult::Error(err);
            }
            None => {
                // Transaction non encore complétée — vérifier timeout.
                if timeout_cycles > 0 {
                    let elapsed = read_tsc().wrapping_sub(start);
                    if elapsed >= timeout_cycles {
                        POLL_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
                        return PollResult::Timeout;
                    }
                }
            }
        }

        // `pause` toutes les SPIN_BATCH_SIZE itérations pour éviter de saturer
        // le port d'exécution des charges mémoire et permettre la progression
        // du thread producteur (hyper-threading).
        batch += 1;
        if batch >= SPIN_BATCH_SIZE {
            batch = 0;
            cpu_pause();
        }
    }
}

/// Variante de `spin_poll` avec un nombre limité d'itérations.
///
/// Retourne `PollResult::Timeout` si la transaction n'est pas terminée
/// après `max_iters` lectures, sans tenir compte du TSC.
///
/// Utile dans les contextes où lire le TSC est interdit.
pub fn bounded_poll(txn_id: DmaTransactionId, max_iters: u32) -> PollResult {
    for i in 0..max_iters {
        match DMA_COMPLETION.query(txn_id) {
            Some(Ok(bytes)) => {
                POLL_STATS.successes.fetch_add(1, Ordering::Relaxed);
                return PollResult::Done(bytes);
            }
            Some(Err(err)) => {
                POLL_STATS.errors.fetch_add(1, Ordering::Relaxed);
                return PollResult::Error(err);
            }
            None => {}
        }
        if i % SPIN_BATCH_SIZE == SPIN_BATCH_SIZE - 1 {
            cpu_pause();
        }
    }
    POLL_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
    PollResult::Timeout
}

/// Polling avec timeout par défaut (`DEFAULT_POLL_TIMEOUT_CYCLES`).
#[inline]
pub fn poll(txn_id: DmaTransactionId) -> PollResult {
    spin_poll(txn_id, DEFAULT_POLL_TIMEOUT_CYCLES)
}

/// Polling avec timeout long (gros transferts).
#[inline]
pub fn poll_long(txn_id: DmaTransactionId) -> PollResult {
    spin_poll(txn_id, LONG_POLL_TIMEOUT_CYCLES)
}

// ─────────────────────────────────────────────────────────────────────────────
// UTILITAIRES
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le TSC.
#[inline(always)]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: rdtsc disponible sur tout x86_64; options(nostack, nomem) correctes.
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    { 0 }
}

/// Instruction `pause` (REPNE NOP) — réduit la contention sur le cache cohérence.
#[inline(always)]
fn cpu_pause() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: pause disponible sur x86_64; réduit la contention de cohérence cache.
    unsafe {
        core::arch::asm!("pause", options(nostack, nomem, preserves_flags));
    }
}
