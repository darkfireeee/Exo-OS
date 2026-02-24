// kernel/src/scheduler/timer/tick.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Tick périodique — HZ=1000 (1ms par tick)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Appelé depuis le gestionnaire d'interruption timer (IRQ0 ou LAPIC timer).
// Effectue, dans l'ordre :
//   1. Mise à jour de l'horloge monotone (statistique)
//   2. Mise à jour du vruntime du thread courant (CFS)
//   3. Vérification de préemption CFS
//   4. Vérification de quantum RR
//   5. Déclenchement des hrtimers expirés
//   6. Équilibrage de charge (tous les BALANCE_INTERVAL_TICKS ticks)
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use core::ptr::NonNull;
use crate::scheduler::core::task::{ThreadControlBlock, SchedPolicy, task_flags, CpuId};
use crate::scheduler::core::runqueue;
use crate::scheduler::policies::{tick_check_preempt, rr_tick, timeslice_for};
use crate::scheduler::smp::load_balance::{balance_cpu, BALANCE_INTERVAL_TICKS};
use super::{hrtimer, clock};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const HZ: u64 = 1000;
pub const TICK_NS: u64 = 1_000_000_000 / HZ;

// Temps accumulé sur le CPU courant depuis la dernière sélection du thread (par CPU).
static ELAPSED_NS: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};

pub static TICK_COUNT:         AtomicU64 = AtomicU64::new(0);
pub static TICK_PREEMPTIONS:   AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Handler principal du tick
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé à chaque tick timer sur le CPU `cpu_id` avec le thread courant.
///
/// # Safety
/// Appelé depuis un contexte d'interruption (préemption implicitement désactivée).
/// Le pointeur `current` doit être valide.
#[no_mangle]
pub unsafe extern "C" fn scheduler_tick(cpu_id: u32, current: *mut ThreadControlBlock) {
    let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);

    let tcb = match NonNull::new(current) {
        Some(p) => &mut *p.as_ptr(),
        None    => return,
    };

    // ── 1. Statistiques CPU (instrumentation globale) ────────────────────
    crate::scheduler::stats::per_cpu::inc_ticks(cpu_id as usize);

    // ── 2. Vruntime CFS ──────────────────────────────────────────────────
    let rq = runqueue::run_queue(CpuId(cpu_id));
    let nr = rq.nr_running_usize();

    // Accumuler l'elapsed sur ce CPU.
    let cpu_idx = (cpu_id as usize).min(255);
    let elapsed = ELAPSED_NS[cpu_idx].fetch_add(TICK_NS, Ordering::Relaxed) + TICK_NS;

    match tcb.policy {
        SchedPolicy::Normal | SchedPolicy::Batch => {
            // Avancer le vruntime du thread pondéré.
            tcb.advance_vruntime(TICK_NS, tcb.priority.cfs_weight());

            // Calculer le timeslice actuel.
            let tw = rq.total_cfs_weight();
            let slice = timeslice_for(tcb, nr, tw);

            // ── 3. Préemption CFS ───────────────────────────────────────
            if tick_check_preempt(tcb, elapsed, slice, nr) {
                tcb.flags.fetch_or(task_flags::NEED_RESCHED, Ordering::Release);
                ELAPSED_NS[cpu_idx].store(0, Ordering::Relaxed);
                TICK_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
            }
        }
        SchedPolicy::RoundRobin => {
            // ── 4. Quantum RR ──────────────────────────────────────────
            if rr_tick(tcb, elapsed) {
                tcb.flags.fetch_or(task_flags::NEED_RESCHED, Ordering::Release);
                ELAPSED_NS[cpu_idx].store(0, Ordering::Relaxed);
                TICK_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
            }
        }
        _ => {}
    }

    // ── 5. Hrtimers ───────────────────────────────────────────────────────
    hrtimer::fire_expired(cpu_id as usize);

    // ── 6. Équilibrage de charge ──────────────────────────────────────────
    if tick % BALANCE_INTERVAL_TICKS == 0 {
        balance_cpu(CpuId(cpu_id));
    }
}

/// Initialise le sous-système tick pour `nr_cpus` CPUs.
///
/// # Safety
/// Appelé une seule fois depuis `scheduler::init()`.
pub unsafe fn init(_nr_cpus: usize) {
    TICK_COUNT.store(0, Ordering::Relaxed);
    TICK_PREEMPTIONS.store(0, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORT — pont pour arch/x86_64/exceptions.rs (handler IPI reschedule)
// ─────────────────────────────────────────────────────────────────────────────
//
// Réponse à l'IPI 0xF1 (reschedule_ipi) envoyé par un autre CPU via
// `arch_send_reschedule_ipi()` dans arch/x86_64/sched_iface.rs.
//
// Action : positionner NEED_RESCHED sur le TCB courant + EOI est géré par arch.
// Le reschedule effectif aura lieu au retour du handler d'interruption,
// lors de la vérification des flags NEED_RESCHED en mode kernel (ou IRET vers
// user space si le code supporte la préemption kernel).
// ─────────────────────────────────────────────────────────────────────────────

/// Pont C ABI pour `do_ipi_reschedule` (arch/x86_64/exceptions.rs).
///
/// `tcb_ptr` : pointeur vers le `ThreadControlBlock` courant (lu depuis GS:[0x20]).
///   - Si null : IPI reçu avant l'init scheduler — ignoré silencieusement.
///   - Sinon  : positionne le flag `NEED_RESCHED` sur ce thread pour
///              déclencher un reschedule au retour d'interruption.
///
/// # Safety
/// Appelé depuis un handler d'interruption, préemption implicitement désactivée.
/// `tcb_ptr`, si non-null, DOIT pointer vers un TCB valide.
#[no_mangle]
pub unsafe extern "C" fn sched_ipi_reschedule(tcb_ptr: *mut u8) {
    let tcb = match NonNull::new(tcb_ptr as *mut ThreadControlBlock) {
        Some(p) => &mut *p.as_ptr(),
        None    => return,  // boot ou idle sans TCB — ignorer
    };
    tcb.flags.fetch_or(task_flags::NEED_RESCHED, Ordering::Release);
    TICK_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
}
