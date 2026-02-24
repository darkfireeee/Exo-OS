// kernel/src/scheduler/policies/ai_guided.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Guidance AI — Tables de lookup statiques pour hints de scheduling
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE IA-KERNEL-01 : Seules des tables de lookup en .rodata sont autorisées
//   dans Ring 0. Aucune inférence réseau de neurones dans le kernel.
//
// RÈGLE IA-KERNEL-02 : Fallback gracieux vers le scheduling déterministe si
//   les hints AI sont absents ou si AI_HINTS_ENABLED == false.
//
// RÈGLE IA-KERNEL-03 : L'entraînement est entièrement séparé dans
//   tools/ai_trainer/ (espace utilisateur).
//
// Fonctionnement :
//   • `ThreadAiState` dans le TCB stocke des EMA (Exponential Moving Averages)
//     de la durée CPU et des pauses I/O, calculés à chaque context_switch.
//   • Ces EMA sont quantifiés en 16 niveaux (0–15) et indexent des tables .rodata
//     qui donnent une note de "préférence" pour chaque combinaison.
//   • `maybe_prefer()` utilise ces tables pour ajuster le choix de la run queue
//     CFS si deux threads ont un vruntime très proche (dans VRUNTIME_CLOSE_NS).
//
// En cas de doute, le candidat fourni par la run queue CFS est retourné tel quel.
// ═══════════════════════════════════════════════════════════════════════════════

use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::scheduler::core::task::{ThreadControlBlock, ThreadAiState};
use crate::scheduler::core::runqueue::PerCpuRunQueue;

// ─────────────────────────────────────────────────────────────────────────────
// Contrôle global
// ─────────────────────────────────────────────────────────────────────────────

/// Active ou désactive la guidance AI (peut être modifié par sysctl).
pub static AI_HINTS_ENABLED: AtomicBool = AtomicBool::new(true);
/// Nombre de fois que la guidance AI a influencé un choix.
pub static AI_GUIDED_PICKS: AtomicU64 = AtomicU64::new(0);
/// Nombre de fois que la guidance AI a cédé au fallback déterministe.
pub static AI_FALLBACK_PICKS: AtomicU64 = AtomicU64::new(0);

/// Seuil de vruntime en-dessous duquel deux threads sont considérés "à égalité"
/// et donc sujets à la guidance AI (1ms).
const VRUNTIME_CLOSE_NS: u64 = 1_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// Tables de lookup (RÈGLE IA-KERNEL-01 : .rodata uniquement)
// ─────────────────────────────────────────────────────────────────────────────

/// Résolution de quantisation des EMA : 16 niveaux (0–15).
const QUANT_LEVELS: usize = 16;

/// Table de pénalité CPU-bound (thread très CPU-bound = pénalisé).
#[link_section = ".rodata"]
static CPU_BOUND_PENALTY: [i8; QUANT_LEVELS] = [
     0,   0,  -1,  -1,  -2,  -2,  -3,  -4,
    -5,  -6,  -7,  -8,  -9, -10, -11, -12,
];

/// Table de bonus IO-bound (thread réactif = favorisé).
#[link_section = ".rodata"]
static IO_BOUND_BONUS: [i8; QUANT_LEVELS] = [
     0,   1,   2,   3,   4,   5,   6,   7,
     8,   9,  10,  11,  12,  12,  12,  12,
];

// ─────────────────────────────────────────────────────────────────────────────
// Quantisation des EMA
// ─────────────────────────────────────────────────────────────────────────────

// Quantization is done inline at call site using bit-shifts.

// ─────────────────────────────────────────────────────────────────────────────
// Score AI
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule un score ai-guidé pour un thread.
///
/// Score positif = favorisé ; score négatif = pénalisé.
/// RÈGLE IA-KERNEL-02 : si les hints sont désactivés, score = 0 (neutre).
fn ai_score(tcb: &ThreadControlBlock) -> i32 {
    if !AI_HINTS_ENABLED.load(Ordering::Relaxed) {
        return 0;
    }
    let ai = &tcb.ai_state;
    // cpu_burst_ema and sleep_ema are u16 — take high byte for 16-level quantization.
    let cpu_level   = (ai.cpu_burst_ema >> 12) as usize;
    let sleep_level = (ai.sleep_ema >> 12) as usize;

    let penalty = CPU_BOUND_PENALTY[cpu_level] as i32;
    let bonus   = IO_BOUND_BONUS[sleep_level] as i32;
    bonus + penalty
}

// ─────────────────────────────────────────────────────────────────────────────
// Interface publique
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé depuis `pick_next.rs` après que la run queue CFS a sélectionné
/// `candidate`. Si un autre thread dans la run queue a un vruntime très proche
/// mais un meilleur score AI, retourne ce thread à la place.
///
/// GARANTIE : ne modifie jamais la structure de la run queue (lecture seule sur
/// les candidats alternatifs). Le choix final est toujours valide.
///
/// # Safety
/// Appelé avec la préemption désactivée (propriétaire de la run queue).
pub unsafe fn maybe_prefer(
    rq: &mut PerCpuRunQueue,
    candidate: NonNull<ThreadControlBlock>,
) -> NonNull<ThreadControlBlock> {
    if !AI_HINTS_ENABLED.load(Ordering::Relaxed) {
        AI_FALLBACK_PICKS.fetch_add(1, Ordering::Relaxed);
        return candidate;
    }

    let cand_ref = candidate.as_ref();
    let cand_vr  = cand_ref.vruntime.load(Ordering::Relaxed);
    let cand_score = ai_score(cand_ref);

    // Cherche un thread alternatif dans la tranche vruntime ± VRUNTIME_CLOSE_NS.
    if let Some(alt) = rq.cfs_peek_second() {
        let alt_ref  = alt.as_ref();
        let alt_vr   = alt_ref.vruntime.load(Ordering::Relaxed);

        // Ne considérer l'alternatif que s'il est "proche" du candidat.
        let delta = if alt_vr > cand_vr { alt_vr - cand_vr } else { cand_vr - alt_vr };
        if delta <= VRUNTIME_CLOSE_NS {
            let alt_score = ai_score(alt_ref);
            if alt_score > cand_score {
                AI_GUIDED_PICKS.fetch_add(1, Ordering::Relaxed);
                return alt;
            }
        }
    }

    AI_FALLBACK_PICKS.fetch_add(1, Ordering::Relaxed);
    candidate
}
