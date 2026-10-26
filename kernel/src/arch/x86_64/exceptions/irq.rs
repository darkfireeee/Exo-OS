#![allow(unused_imports, dead_code, clippy::missing_safety_doc)]
use super::*;
use super::frame::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};


/// # Safety
/// `frame` doit être un pointeur valide vers une ExceptionFrame allouée par le stub ASM.
#[inline(always)]
pub unsafe fn do_irq_generic(frame: *mut ExceptionFrame, vector: u8) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    crate::arch::x86_64::idt::irq_counter_inc(vector);

    // Routage vers l'architecture GI-03
    crate::arch::x86_64::irq::routing::dispatch_irq(vector, None);

    // Conserver la même sémantique de retour userspace que les autres handlers.
    if frame.from_userspace() {
        exception_return_to_user(frame);
    }
}

macro_rules! define_generic_irq_handler {
    ($name:ident, $vector:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name(frame: *mut ExceptionFrame) {
            do_irq_generic(frame, $vector);
        }
    };
}

define_generic_irq_handler!(do_irq_1, 33);
define_generic_irq_handler!(do_irq_2, 34);
define_generic_irq_handler!(do_irq_3, 35);
define_generic_irq_handler!(do_irq_4, 36);
define_generic_irq_handler!(do_irq_5, 37);
define_generic_irq_handler!(do_irq_6, 38);
define_generic_irq_handler!(do_irq_7, 39);
define_generic_irq_handler!(do_irq_8, 40);
define_generic_irq_handler!(do_irq_9, 41);
define_generic_irq_handler!(do_irq_10, 42);
define_generic_irq_handler!(do_irq_11, 43);
define_generic_irq_handler!(do_irq_12, 44);
define_generic_irq_handler!(do_irq_13, 45);
define_generic_irq_handler!(do_irq_14, 46);
define_generic_irq_handler!(do_irq_15, 47);

/// Handler IRQ timer (vecteur 32, APIC timer)
#[no_mangle]
pub unsafe extern "C" fn do_irq_timer(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    crate::arch::x86_64::idt::irq_counter_inc(32);
    TIMER_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);

    // 1. EOI APIC — acquitté en premier pour minimiser la latence APIC.
    // SAFETY: LAPIC initialisé avant que les IRQ timer soient activées.
    crate::arch::x86_64::apic::eoi();

    if crate::arch::x86_64::apic::local_apic::lapic_timer_owner()
        == crate::arch::x86_64::apic::local_apic::LapicTimerOwner::ExoNmiWatchdog
    {
        crate::security::exonmi::tick();
        return;
    }

    crate::arch::x86_64::apic::local_apic::rearm_scheduler_timer_tick();

    // 2. Tick scheduler : avance les quantum CPU et décide des préemptions.
    let cpu_id = current_cpu_id_checked();
    let tcb_ptr = current_tcb_raw_checked();
    // SAFETY: scheduler_tick est thread-safe ; cli implicite dans handler IRQ.
    unsafe {
        scheduler_tick(cpu_id, tcb_ptr as *mut u8);
    }

    // 3. Si retour vers Ring 3 : vérifier préemption + signaux.
    if frame.from_userspace() {
        exception_return_to_user(frame);
    }
}

/// Handler IRQ spurious (vecteur 0xFF)
///
/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_irq_spurious(_frame: *mut ExceptionFrame) {
    // IRQ spurious : ne PAS envoyer EOI (c'est une fausse interruption)
    SPURIOUS_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ── IPI Handlers ──────────────────────────────────────────────────────────────

/// IPI wakeup thread
///
/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_wakeup(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_WAKEUP);
    // Réutiliser sched_ipi_reschedule : positionne NEED_RESCHED sur le thread
    // courant (RÈGLE IPI-01) — le reschedule effectif a lieu à l'IRET.
    let tcb_ptr = current_tcb_raw_checked();
    // SAFETY: sched_ipi_reschedule est thread-safe pour le CPU courant.
    unsafe {
        sched_ipi_reschedule(tcb_ptr as *mut u8);
    }
    // EOI Local APIC — acquitter l'IPI avant le retour d'interruption.
    // SAFETY: LAPIC initialisé avant tout IPI SMP.
    crate::arch::x86_64::apic::eoi();
}

/// IPI reschedule
///
/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_reschedule(frame: *mut ExceptionFrame) {
    let _ = frame;
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_RESCHEDULE);

    let tcb_ptr = current_tcb_raw_checked();

    // Positionner NEED_RESCHED sur le thread courant (RÈGLE IPI-01 DOC3).
    // Le reschedule effectif aura lieu au retour d'interruption quand le code
    // kernel vérifiera le flag (ou à l'IRET vers Ring 3).
    // Si le scheduler n'est pas encore initialisé (tcb_ptr == 0), ignoré.
    // SAFETY: sched_ipi_reschedule() est thread-safe pour le CPU courant.
    unsafe {
        sched_ipi_reschedule(tcb_ptr as *mut u8);
    }

    // EOI Local APIC — acquitter l'IPI avant le retour d'interruption.
    // SAFETY: LAPIC initialisé avant tout IPI SMP.
    crate::arch::x86_64::apic::eoi();
}

/// IPI TLB shootdown
///
/// ## Intégration memory/ (RÈGLE TLB-01 DOC2)
/// 1. Récupérer l'identifiant CPU courant depuis le GS per-CPU.
/// 2. Appeler `TLB_QUEUE.handle_remote(cpu_id)` qui :
///    - Lit la requête de flush depuis la queue TLB partagée.
///    - Exécute `invlpg` ou reload CR3 selon `TlbFlushType`.
///    - Détache le bit correspondant dans le champ `completed`.
/// 3. EOI Local APIC (acquitter l'interruption).
///
/// ## Règle de précédence
/// JAMAIS appeler free_pages() avant que TOUS les CPUs aient acquitté
/// (c'est `tlb::shootdown()` qui attend l'acquittement, pas ce handler).
///
/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_tlb_shootdown(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_TLB_SHOOTDOWN);
    crate::arch::x86_64::paging::inc_tlb_shootdown();

    // Identifiant logique du CPU courant (0-based).
    let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id() as u8;

    // Exécuter le flush TLB demandé par l'émetteur du shootdown.
    // SAFETY: appelé depuis handler IRQ (cli implicite); TLB_QUEUE thread-safe par spinlock.
    unsafe {
        crate::memory::virt::address_space::tlb::TLB_QUEUE.handle_remote(cpu_id);
    }

    // EOI Local APIC — signale la fin du traitement de l'interruption.
    // SAFETY: LAPIC initialisé avant tout IPI ; EOI est idempotent à ce stade.
    crate::arch::x86_64::apic::eoi();
}

/// IPI hotplug CPU
///
/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_cpu_hotplug(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_IPI_CPU_HOTPLUG);

    // EOI avant halt pour que le BSP ne reste pas bloqué en attente.
    // SAFETY: LAPIC initialisé avant tout IPI hotplug.
    crate::arch::x86_64::apic::eoi();
    // Identifier ce CPU et le mettre hors ligne (→ ! — ce CPU ne revient pas).
    let cpu_id = crate::arch::x86_64::smp::percpu::current_cpu_id();
    // SAFETY: hotplug_cpu_halt est idempotent et -> ! (arrêt irréversible).
    crate::arch::x86_64::smp::hotplug::hotplug_cpu_halt(cpu_id);
}

/// IPI panic broadcast (0xFE)
///
/// Reçu par tous les APs lors d'un kernel panic.
/// Arrêt immédiat sans tentative de sauvegarde.
///
/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_ipi_panic(_frame: *mut ExceptionFrame) {
    // SAFETY: réponse à un panic — arrêt irréversible requis
    crate::arch::x86_64::halt_cpu();
}

/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_exophoenix_freeze(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_EXOPHOENIX_FREEZE);
    // SAFETY: handler d'IPI ExoPhoenix FREEZE en contexte interruption Ring 0.
    unsafe { crate::exophoenix::interrupts::handle_freeze_ipi() };
}

/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_exophoenix_pmc(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_EXOPHOENIX_PMC);
    // SAFETY: handler d'IPI ExoPhoenix PMC snapshot en contexte interruption Ring 0.
    unsafe { crate::exophoenix::interrupts::handle_pmc_snapshot_ipi() };
}

/// # Safety
/// Appelé exclusivement depuis le stub ASM d'interruption.
#[no_mangle]
pub unsafe extern "C" fn do_exophoenix_tlb(_frame: *mut ExceptionFrame) {
    crate::arch::x86_64::idt::irq_counter_inc(crate::arch::x86_64::idt::VEC_EXOPHOENIX_TLB);
    // SAFETY: handler d'IPI ExoPhoenix TLB flush en contexte interruption Ring 0.
    unsafe { crate::exophoenix::interrupts::handle_tlb_flush_ipi() };
}

