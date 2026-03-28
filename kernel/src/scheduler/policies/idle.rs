// kernel/src/scheduler/policies/idle.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Politique SCHED_IDLE — Tâche de fond et boucle HLT
// ═══════════════════════════════════════════════════════════════════════════════
//
// La tâche idle est la tâche permanente de dernier recours sur chaque CPU.
// Elle tourne uniquement quand la run queue est vide.
//
// Comportement :
//  1. Vérifie si un thread est en attente dans la run queue.
//  2. Si oui → cède immédiatement (reschedule).
//  3. Sinon  → exécute `hlt` pour économiser l'énergie.
//
// La boucle idle est aussi le point de délégation vers energy::c_states
// pour choisir le C-state optimal selon l'inactivité prévue.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::core::task::{ThreadControlBlock, SCHED_IDLE_BIT};

// ─────────────────────────────────────────────────────────────────────────────
// Métriques idle
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de fois que le CPU est entré dans la boucle idle.
pub static IDLE_ENTRIES: AtomicU64 = AtomicU64::new(0);
/// Nombre d'itérations de la boucle idle (= HLT exécutés).
pub static IDLE_HLT_COUNT: AtomicU64 = AtomicU64::new(0);
/// Nombre de fois que la boucle idle a détecté du travail et cedé.
pub static IDLE_WAKEUPS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Marquage de la tâche idle
// ─────────────────────────────────────────────────────────────────────────────

/// Marque un TCB comme étant la tâche idle de son CPU.
pub fn mark_idle_thread(tcb: &mut ThreadControlBlock) {
    tcb.sched_state.fetch_or(SCHED_IDLE_BIT, Ordering::Relaxed);
}

/// Retourne `true` si le TCB est la tâche idle.
pub fn is_idle_thread(tcb: &ThreadControlBlock) -> bool {
    tcb.is_idle()
}

// ─────────────────────────────────────────────────────────────────────────────
// Point d'entrée de la boucle idle
// ─────────────────────────────────────────────────────────────────────────────

/// Une itération de la boucle idle.
///
/// Doit être appelée en boucle depuis la tâche idle créée par le kernel init.
/// Retourne `true` si du travail a été détecté (le scheduler devrait reprendre
/// la main), `false` si on a dormi via HLT.
///
/// # Safety
/// Appelé avec la préemption activée et les IRQ activées.
pub unsafe fn idle_iteration(nr_running: usize) -> bool {
    if nr_running > 0 {
        // Travail disponible — signaler NEED_RESCHED et redonner la main.
        IDLE_WAKEUPS.fetch_add(1, Ordering::Relaxed);
        return true;
    }

    // Aucun thread prêt — exécuter HLT (attend la prochaine IRQ).
    IDLE_HLT_COUNT.fetch_add(1, Ordering::Relaxed);
    core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));

    // Après le retour de HLT (une IRQ a été reçue), laisser le scheduler décider.
    false
}

/// Point d'entrée principal pour la tâche idle d'un CPU.
///
/// Initialise les métriques puis boucle indéfiniment.
/// Cette fonction ne doit jamais retourner.
///
/// # Safety
/// Appelé uniquement depuis la task idle de chaque CPU.
pub unsafe fn idle_loop(get_nr_running: unsafe fn() -> usize) -> ! {
    IDLE_ENTRIES.fetch_add(1, Ordering::Relaxed);
    loop {
        let nr = get_nr_running();
        idle_iteration(nr);
    }
}
