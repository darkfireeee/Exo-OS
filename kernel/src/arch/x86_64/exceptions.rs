//! # arch/x86_64/exceptions.rs — Handlers d'exceptions CPU
//!
//! Implémente les handlers pour les 32 exceptions x86_64 (vecteurs 0–31),
//! les IRQ hardware, et les IPIs scheduler.
//!
//! ## Règle DOC1 — Point de retour userspace après préemption
//! `exception_return_to_user()` est le SECOND point d'orchestration des signaux
//! (avec `syscall_return_to_user()`). Après toute exception depuis Ring 3,
//! arch/ vérifie `signal_pending` et orchestre la livraison.
//!
//! ## Séquence handler d'exception
//! 1. Sauvegarder registres (PUSH dans ASM entry)
//! 2. SWAPGS si Ring 3 (vérifier CS saved sur la pile)
//! 3. Appeler handler Rust
//! 4. `exception_return_to_user()` si retour vers Ring 3
//! 5. SWAPGS si Ring 3
//! 6. IRETQ


use core::concat;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Ponts C ABI vers le scheduler (RÈGLE FPU-02 + RÈGLE IPI-01 DOC3) ─────────
//
// arch/ ne peut pas importer scheduler/ directement (éviter cycle de dépendance
// Rust crate-level). Les deux fonctions ci-dessous sont exportées par le
// scheduler via `#[no_mangle] pub unsafe extern "C"` et résolues à l'édition
// des liens.
//
// sched_fpu_handle_nm   — scheduler/fpu/lazy.rs     — handler #NM (FPU lazy)
// sched_ipi_reschedule  — scheduler/timer/tick.rs   — IPI reschedule 0xF1
extern "C" {
    /// Gère l'exception #NM (Device Not Available) pour le TCB courant.
    /// Efface CR0.TS et restaure le contexte FPU via xrstor.
    /// `tcb_ptr` = GS:[0x20] (current_tcb). Si null → simple clts.
    fn sched_fpu_handle_nm(tcb_ptr: *mut u8);
    /// Positionne NEED_RESCHED sur le thread courant suite à un IPI reschedule.
    /// `tcb_ptr` = GS:[0x20] (current_tcb). Si null → ignoré.
    fn sched_ipi_reschedule(tcb_ptr: *mut u8);
    /// Tick du scheduler timer : avance les quantum, décide préemption.
    /// `cpu_id` = GS:[0x10], `current` = GS:[0x20].
    fn scheduler_tick(cpu_id: u32, current: *mut u8);
    /// Pont arch/→process/ : livraison signaux au retour exception Ring 3.
    /// `tcb_ptr` = GS:[0x20], `excframe` = &mut ExceptionFrame courant.
    fn proc_signal_on_exception_return(tcb_ptr: *mut u8, excframe: *mut u8);
}

// ── Frame d'exception ─────────────────────────────────────────────────────────

/// Registres sauvegardés par le CPU + l'ASM de stub lors d'une exception
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ExceptionFrame {
    // Registres sauvegardés par l'ASM stub (caller-saved)
    pub r15:    u64,
    pub r14:    u64,
    pub r13:    u64,
    pub r12:    u64,
    pub r11:    u64,
    pub r10:    u64,
    pub r9:     u64,
    pub r8:     u64,
    pub rbp:    u64,
    pub rdi:    u64,
    pub rsi:    u64,
    pub rdx:    u64,
    pub rcx:    u64,
    pub rbx:    u64,
    pub rax:    u64,

    // Poussés par le stub ou le CPU selon l'exception
    pub error_code: u64,

    // Poussés automatiquement par le CPU
    pub rip:    u64,
    pub cs:     u64,
    pub rflags: u64,
    pub rsp:    u64,   // Présent uniquement si changement de niveau de privilège
    pub ss:     u64,   // Idem
}

impl ExceptionFrame {
    /// Retourne `true` si l'exception vient de Ring 3 (userspace)
    #[inline(always)]
    pub fn from_userspace(&self) -> bool {
        (self.cs & 3) == 3
    }

    /// Retourne `true` si l'exception vient du kernel (Ring 0)
    #[inline(always)]
    pub fn from_kernel(&self) -> bool {
        (self.cs & 3) == 0
    }
}

// ── Macros ASM pour les stubs d'interruption ──────────────────────────────────

macro_rules! define_exception_handler_errcode {
    ($name:ident, $rust_handler:ident) => {
        core::arch::global_asm!(
            concat!(".global ", stringify!($name)),
            concat!(".type ", stringify!($name), ", @function"),
            concat!(stringify!($name), ":"),
            // À l'entrée du handler, le CPU a pousé (de bas à haut) :
            // [rsp+ 0] = error_code
            // [rsp+ 8] = RIP
            // [rsp+16] = CS  ← RPL bits [1:0] = ring level
            // [rsp+24] = RFLAGS
            // [rsp+32] = RSP  (si changement Ring 3→Ring 0)
            // [rsp+40] = SS   (si changement Ring 3→Ring 0)
            "test qword ptr [rsp + 16], 3",  // CS & 3 != 0 ⇒ Ring 3
            "jz   1f",                        // ZF=1 ⇒ Ring 0 (kernel), sauter swapgs
            "swapgs",
            "1:",
            // Sauvegarder registres callee-saved + scratch
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push rbp",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            // ExceptionFrame* dans rdi
            "mov  rdi, rsp",
            concat!("call ", stringify!($rust_handler)),
            // Restaurer registres
            "pop  r15",
            "pop  r14",
            "pop  r13",
            "pop  r12",
            "pop  r11",
            "pop  r10",
            "pop  r9",
            "pop  r8",
            "pop  rbp",
            "pop  rdi",
            "pop  rsi",
            "pop  rdx",
            "pop  rcx",
            "pop  rbx",
            "pop  rax",
            // Dépiler error_code
            "add  rsp, 8",
            // Après add rsp,8 : [rsp+0]=RIP, [rsp+8]=CS, [rsp+16]=RFLAGS
            // Vérifier CS RPL pour le SWAPGS de sortie
            "test qword ptr [rsp + 8], 3",   // CS & 3 != 0 ⇒ retour Ring 3
            "jz   2f",
            "swapgs",
            "2:",
            "iretq",
        );
    }
}

/// Génère un stub d'entrée exception SANS error code (pousse 0 synthétique)
///
/// Après "push 0" (synthetic error_code), la pile est identique au cas errcode :
/// [rsp+ 0] = 0  (erreur synthétique)
/// [rsp+ 8] = RIP
/// [rsp+16] = CS  ← mêmes offsets que le cas errcode
macro_rules! define_exception_handler_no_errcode {
    ($name:ident, $rust_handler:ident) => {
        core::arch::global_asm!(
            concat!(".global ", stringify!($name)),
            concat!(".type ", stringify!($name), ", @function"),
            concat!(stringify!($name), ":"),
            // Pousser 0 comme error_code synthétique
            "push 0",
            // [rsp+ 0]=0, [rsp+ 8]=RIP, [rsp+16]=CS, [rsp+24]=RFLAGS ...
            "test qword ptr [rsp + 16], 3",  // CS & 3 != 0 ⇒ Ring 3
            "jz   1f",
            "swapgs",
            "1:",
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push rbp",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "mov  rdi, rsp",
            concat!("call ", stringify!($rust_handler)),
            "pop  r15",
            "pop  r14",
            "pop  r13",
            "pop  r12",
            "pop  r11",
            "pop  r10",
            "pop  r9",
            "pop  r8",
            "pop  rbp",
            "pop  rdi",
            "pop  rsi",
            "pop  rdx",
            "pop  rcx",
            "pop  rbx",
            "pop  rax",
            "add  rsp, 8",
            // Après add rsp,8 : [rsp+0]=RIP, [rsp+8]=CS
            "test qword ptr [rsp + 8], 3",   // CS RPL bits
            "jz   2f",
            "swapgs",
            "2:",
            "iretq",
        );
    }
}

// Stubs ASM pour toutes les exceptions
define_exception_handler_no_errcode!(exc_divide_error_handler,     do_divide_error);
define_exception_handler_no_errcode!(exc_debug_handler,            do_debug);
define_exception_handler_no_errcode!(exc_nmi_handler,              do_nmi);
define_exception_handler_no_errcode!(exc_breakpoint_handler,       do_breakpoint);
define_exception_handler_no_errcode!(exc_overflow_handler,         do_overflow);
define_exception_handler_no_errcode!(exc_bound_range_handler,      do_bound_range);
define_exception_handler_no_errcode!(exc_invalid_opcode_handler,   do_invalid_opcode);
define_exception_handler_no_errcode!(exc_device_not_avail_handler, do_device_not_avail);
define_exception_handler_errcode!(   exc_double_fault_handler,     do_double_fault);
define_exception_handler_errcode!(   exc_invalid_tss_handler,      do_invalid_tss);
define_exception_handler_errcode!(   exc_segment_not_present_handler, do_segment_not_present);
define_exception_handler_errcode!(   exc_stack_fault_handler,      do_stack_fault);
define_exception_handler_errcode!(   exc_general_protection_handler, do_general_protection);
define_exception_handler_errcode!(   exc_page_fault_handler,       do_page_fault);
define_exception_handler_no_errcode!(exc_x87_fp_handler,           do_x87_fp);
define_exception_handler_errcode!(   exc_alignment_check_handler,  do_alignment_check);
define_exception_handler_no_errcode!(exc_machine_check_handler,    do_machine_check);
define_exception_handler_no_errcode!(exc_simd_fp_handler,          do_simd_fp);
define_exception_handler_no_errcode!(exc_virtualization_handler,   do_virtualization);
define_exception_handler_errcode!(   exc_ctrl_protection_handler,  do_ctrl_protection);

// IRQ et IPI
define_exception_handler_no_errcode!(irq_timer_handler,          do_irq_timer);
define_exception_handler_no_errcode!(irq_spurious_handler,       do_irq_spurious);
define_exception_handler_no_errcode!(ipi_wakeup_handler,         do_ipi_wakeup);
define_exception_handler_no_errcode!(ipi_reschedule_handler,     do_ipi_reschedule);
define_exception_handler_no_errcode!(ipi_tlb_shootdown_handler,  do_ipi_tlb_shootdown);
define_exception_handler_no_errcode!(ipi_cpu_hotplug_handler,    do_ipi_cpu_hotplug);
define_exception_handler_no_errcode!(ipi_panic_handler,          do_ipi_panic);

// ── Handlers Rust d'exceptions ────────────────────────────────────────────────

/// Retour vers userspace après exception depuis Ring 3
///
/// ## RÈGLE SIGNAL-01 (DOC1)
/// Point d'orchestration des signaux après préemption ou exception.
/// Vérifie `signal_pending` dans le TCB et orchestre la livraison.
/// La livraison effective est déléguée à `process::signal::delivery`.
fn exception_return_to_user(frame: &mut ExceptionFrame) {
    // Lire le TCB courant depuis GS:[0x20] (PerCpuData::current_tcb).
    let tcb_ptr: u64;
    // SAFETY: GS initialisé par percpu::init() avant tout handler d'exception.
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nostack, nomem),
        );
    }
    if tcb_ptr == 0 { return; }
    // SAFETY: proc_signal_on_exception_return thread-safe; exclu par cli implicite dans handler.
    unsafe {
        proc_signal_on_exception_return(
            tcb_ptr as *mut u8,
            frame as *mut ExceptionFrame as *mut u8,
        );
    }
}

/// Handler #DE — Division par zéro
#[no_mangle]
extern "C" fn do_divide_error(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` est un pointeur non-null passé par le stub ASM, aligné 16 B,
    // unique pour ce contexte d'exception. Sa durée de vie est garantie par la
    // frame de pile que le CPU a poussée avant l'appel au handler.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[0].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // Envoyer SIGFPE au processus courant via exception_return_to_user
        // (proc_signal_on_exception_return → process::signal::delivery — RÈGLE SIGNAL-01)
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Division par zéro kernel", frame);
    }
}

/// Handler #DB — Debug exception
#[no_mangle]
extern "C" fn do_debug(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[1].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGTRAP → ptrace/GDB
        exception_return_to_user(frame);
    }
    // Kernel debug : ignorer silencieusement (breakpoint hardware noyau)
}

/// Handler #NMI — Non-Maskable Interrupt
///
/// NMI utilise IST2 (pile dédiée indépendante de la pile courante).
/// NE PAS appeler de code qui pourrait provoquer une exception.
#[no_mangle]
extern "C" fn do_nmi(frame: *mut ExceptionFrame) {
    let _ = frame;
    EXC_COUNTERS[2].fetch_add(1, Ordering::Relaxed);
    NMI_COUNT.fetch_add(1, Ordering::Relaxed);

    // NMI handler : vérifier watchdog, hpet, mce
    // Minimal : incrémenter compteur sans allocation ni verrou
}

/// Handler #BP — Breakpoint (INT3)
#[no_mangle]
extern "C" fn do_breakpoint(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[3].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGTRAP
        exception_return_to_user(frame);
    }
    // Kernel : kprobe ou debug noyau
}

/// Handler #OF — Overflow (INTO)
#[no_mangle]
extern "C" fn do_overflow(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[4].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() { exception_return_to_user(frame); }
    else { kernel_panic_exception("Overflow kernel", frame); }
}

/// Handler #BR — Bound Range Exceeded
#[no_mangle]
extern "C" fn do_bound_range(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[5].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() { exception_return_to_user(frame); }
    else { kernel_panic_exception("Bound Range kernel", frame); }
}

/// Handler #UD — Invalid Opcode
#[no_mangle]
extern "C" fn do_invalid_opcode(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[6].fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGILL
        exception_return_to_user(frame);
    } else {
        // Vérifier si c'est une instruction XTEST (TSX) — retpoline parfois génère #UD
        // Pour l'instant : kernel panic
        kernel_panic_exception("Invalid Opcode kernel", frame);
    }
}

/// Handler #NM — Device Not Available (FPU lazy)
///
/// Déclenché quand CR0.TS=1 et qu'un thread tente d'utiliser la FPU.
/// scheduler/fpu/lazy.rs gère la logique — arch/ gère l'exception.
#[no_mangle]
extern "C" fn do_device_not_avail(frame: *mut ExceptionFrame) {
    let _ = frame;
    EXC_COUNTERS[7].fetch_add(1, Ordering::Relaxed);
    FPU_DEVICE_NOT_AVAIL_COUNT.fetch_add(1, Ordering::Relaxed);

    // Lire le TCB courant depuis la donnée per-CPU partagée.
    // GS:[0x20] = `current_tcb: u64` dans la structure PerCpuData (percpu.rs).
    let tcb_ptr: u64;
    // SAFETY: GS initialisé par percpu::init() avant tout traitement d'interruption.
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nostack, nomem),
        );
    }

    // Déléguer au scheduler (RÈGLE FPU-02 DOC3) :
    //   • Efface CR0.TS (autorise l'accès FPU/SIMD).
    //   • Appelle xrstor pour restaurer le contexte FPU du thread courant.
    //   • Met à jour fpu_loaded = true dans le TCB.
    // Si tcb_ptr == 0 (boot / idle) : simple clts sans xrstor.
    // SAFETY: sched_fpu_handle_nm() est thread-safe pour le CPU courant.
    unsafe { sched_fpu_handle_nm(tcb_ptr as *mut u8); }
}

/// Handler #DF — Double Fault
///
/// Utilise IST1 (pile dédiée). Si on arrive ici, la pile kernel principale
/// est probablement corrompue ou overflow.
///
/// NE PAS allouer, NE PAS prendre de verrous.
#[no_mangle]
extern "C" fn do_double_fault(frame: *mut ExceptionFrame) {
    let _ = frame;
    EXC_COUNTERS[8].fetch_add(1, Ordering::Relaxed);

    // RÈGLE NO-ALLOC : cette fonction ne peut faire aucune allocation
    // Elle ne peut qu'arrêter le CPU
    // SAFETY: situation non-récupérable — halt immédiat
    super::halt_cpu();
}

/// Handler #TS — Invalid TSS
#[no_mangle]
extern "C" fn do_invalid_tss(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[10].fetch_add(1, Ordering::Relaxed);
    kernel_panic_exception("Invalid TSS", frame);
}

/// Handler #NP — Segment Not Present
#[no_mangle]
extern "C" fn do_segment_not_present(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[11].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        // SIGBUS
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Segment Not Present kernel", frame);
    }
}

/// Handler #SS — Stack Fault
#[no_mangle]
extern "C" fn do_stack_fault(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[12].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("Stack Fault kernel", frame);
    }
}

/// Handler #GP — General Protection Fault
#[no_mangle]
extern "C" fn do_general_protection(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[13].fetch_add(1, Ordering::Relaxed);
    GP_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);

    if frame.from_userspace() {
        // SIGSEGV
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#GP kernel", frame);
    }
}

/// Handler #PF — Page Fault
///
/// Dispatcher vers les handlers de la couche memory/ :
/// - CoW break              → memory::virt::fault::cow
/// - Demand paging          → memory::virt::fault::demand_paging
/// - Swap-in                → memory::virt::fault::swap_in
/// - Violation d'accès      → SIGSEGV (Ring 3) ou KernelFault (Ring 0)
///
/// ## Intégration memory/ (RÈGLE MEM-01 DOC2)
/// `memory::virt::fault::handler::handle_page_fault()` est le seul point
/// d'entrée pour tous les faults. Il prend un `FaultContext` construit ici
/// et un `FaultAllocator` (implémenté par `KernelFaultAllocator`).
///
/// ## Intégration process/ (RÈGLE DOC1)
/// Quand process/ sera intégré, l'allocateur utilisera l'espace d'adressage
/// du processus courant. Pour l'instant, `KernelFaultAllocator` est utilisé.
#[no_mangle]
extern "C" fn do_page_fault(frame: *mut ExceptionFrame) {
    // SAFETY: `frame` est un pointeur non-null passé par le stub ASM, aligné 16 B,
    // unique pour ce contexte d'exception.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[14].fetch_add(1, Ordering::Relaxed);
    super::paging::inc_page_fault();

    // CR2 contient l'adresse virtuelle qui a causé le fault.
    let fault_addr_raw = super::read_cr2();
    let error_code     = frame.error_code;

    // Décomposition de l'error code x86_64 :
    // bit 0 = P  (page présente — protection violation)
    // bit 1 = W  (écriture)
    // bit 2 = U  (depuis Ring 3)
    // bit 3 = RSVD (bit réservé corrompu dans le PTE)
    // bit 4 = I  (instruction fetch)
    // bit 5 = PK (protection key violation)
    let is_present     = error_code & 1 != 0;
    let is_write       = error_code & 2 != 0;
    let _is_user        = error_code & 4 != 0;
    let is_instr_fetch = error_code & 16 != 0;
    let _ = is_present; // Utilisé implicitement via FaultCause + FaultContext

    // Construire le FaultContext
    use crate::memory::core::VirtAddr;
    use crate::memory::virt::fault::{FaultCause, FaultContext, FaultResult};
    use super::memory_iface::KERNEL_FAULT_ALLOC;

    let cause = if is_instr_fetch {
        FaultCause::Execute
    } else if is_write {
        FaultCause::Write
    } else {
        FaultCause::Read
    };

    let fault_addr = VirtAddr::new(fault_addr_raw);
    let from_kernel = frame.from_kernel();
    let ctx = FaultContext::new(fault_addr, cause, from_kernel);

    // Dispatcher vers le sous-système memory/
    let result = crate::memory::virt::fault::handler::handle_page_fault(
        &ctx,
        &KERNEL_FAULT_ALLOC,
    );

    match result {
        FaultResult::Handled => {
            // Fault résolu (demand paging, CoW, swap-in) — reprendre l'exécution.
            if frame.from_userspace() {
                exception_return_to_user(frame);
            }
            // En mode kernel : retour direct via IRETQ (stub ASM)
        }
        FaultResult::Segfault { addr } => {
            // Violation d'accès mémoire.
            let _ = addr;
            if frame.from_userspace() {
                // SIGSEGV sera livré par exception_return_to_user (RÈGLE SIGNAL-01).
                // Quand process/ est intégré : process::signal::send(SIGSEGV).
                exception_return_to_user(frame);
            } else {
                kernel_panic_exception("#PF kernel : accès invalide", frame);
            }
        }
        FaultResult::Oom { addr } => {
            // Out of memory — OOM killer notifié.
            let _ = addr;
            if frame.from_userspace() {
                // Le processus sera tué par l'OOM killer asynchrone.
                exception_return_to_user(frame);
            } else {
                kernel_panic_exception("#PF kernel : OOM", frame);
            }
        }
        FaultResult::KernelFault { addr } => {
            // Fault kernel non récupérable.
            let _ = addr;
            kernel_panic_exception("#PF kernel : non récupérable", frame);
        }
    }
}


/// Handler #MF — x87 FP Exception
#[no_mangle]
extern "C" fn do_x87_fp(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[16].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() { exception_return_to_user(frame); }
    else { kernel_panic_exception("#MF x87 kernel", frame); }
}

/// Handler #AC — Alignment Check
#[no_mangle]
extern "C" fn do_alignment_check(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[17].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() { exception_return_to_user(frame); }
    else { kernel_panic_exception("#AC kernel", frame); }
}

/// Handler #MC — Machine Check
///
/// Utilise IST3 (pile dédiée).
/// Ne peut pas se récupérer. Arrêt immédiat.
#[no_mangle]
extern "C" fn do_machine_check(_frame: *mut ExceptionFrame) {
    EXC_COUNTERS[18].fetch_add(1, Ordering::Relaxed);
    MC_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: machine check = hardware non-récupérable
    super::halt_cpu();
}

/// Handler #XM — SIMD FP Exception
#[no_mangle]
extern "C" fn do_simd_fp(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[19].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() { exception_return_to_user(frame); }
    else { kernel_panic_exception("#XM SIMD FP kernel", frame); }
}

/// Handler #VE — Virtualization Exception
#[no_mangle]
extern "C" fn do_virtualization(frame: *mut ExceptionFrame) {
    let _ = frame;
    EXC_COUNTERS[20].fetch_add(1, Ordering::Relaxed);
    // Intel EPT Violation — géré par le module virt/ si VMX actif
}

/// Handler #CP — Control Protection Exception (CET)
#[no_mangle]
extern "C" fn do_ctrl_protection(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[21].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        // CET violation en userspace → SIGSEGV+SEGV_CPERR
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#CP Control Protection kernel", frame);
    }
}

// ── IRQ Handlers ──────────────────────────────────────────────────────────────

/// Handler IRQ timer (vecteur 32, APIC timer)
#[no_mangle]
extern "C" fn do_irq_timer(frame: *mut ExceptionFrame) {
    // SAFETY: identique à do_divide_error — pointeur valide passé par le stub ASM.
    let frame = unsafe { &mut *frame };
    super::idt::irq_counter_inc(32);
    TIMER_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);

    // 1. EOI APIC — acquitté en premier pour minimiser la latence APIC.
    // SAFETY: LAPIC initialisé avant que les IRQ timer soient activées.
    super::apic::eoi();

    // 2. Tick scheduler : avance les quantum CPU et décide des préemptions.
    let cpu_id: u32;
    let tcb_ptr: u64;
    // SAFETY: GS initialisé par percpu::init() avant tout IRQ timer.
    unsafe {
        core::arch::asm!(
            "mov {:e}, gs:[0x10]",
            out(reg) cpu_id,
            options(nostack, nomem),
        );
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nostack, nomem),
        );
    }
    // SAFETY: scheduler_tick est thread-safe ; cli implicite dans handler IRQ.
    unsafe { scheduler_tick(cpu_id, tcb_ptr as *mut u8); }

    // 3. Si retour vers Ring 3 : vérifier préemption + signaux.
    if frame.from_userspace() {
        exception_return_to_user(frame);
    }
}

/// Handler IRQ spurious (vecteur 0xFF)
#[no_mangle]
extern "C" fn do_irq_spurious(_frame: *mut ExceptionFrame) {
    // IRQ spurious : ne PAS envoyer EOI (c'est une fausse interruption)
    SPURIOUS_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ── IPI Handlers ──────────────────────────────────────────────────────────────

/// IPI wakeup thread (0xF0)
#[no_mangle]
extern "C" fn do_ipi_wakeup(_frame: *mut ExceptionFrame) {
    super::idt::irq_counter_inc(0xF0);
    // Réutiliser sched_ipi_reschedule : positionne NEED_RESCHED sur le thread
    // courant (RÈGLE IPI-01) — le reschedule effectif a lieu à l'IRET.
    let tcb_ptr: u64;
    // SAFETY: GS initialisé par percpu::init() avant tout IPI.
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nostack, nomem),
        );
    }
    // SAFETY: sched_ipi_reschedule est thread-safe pour le CPU courant.
    unsafe { sched_ipi_reschedule(tcb_ptr as *mut u8); }
    // EOI Local APIC — acquitter l'IPI avant le retour d'interruption.
    // SAFETY: LAPIC initialisé avant tout IPI SMP.
    super::apic::eoi();
}

/// IPI reschedule (0xF1)
#[no_mangle]
extern "C" fn do_ipi_reschedule(frame: *mut ExceptionFrame) {
    let _ = frame;
    super::idt::irq_counter_inc(0xF1);

    // Mode ExoPhoenix : 0xF1 = Freeze coopératif lock-free.
    if crate::exophoenix::stage0::exophoenix_vectors_active() {
        // SAFETY: handler dédié no-alloc/no-lock, peut diverger volontairement.
        unsafe { crate::exophoenix::interrupts::handle_freeze_ipi() };
    }

    // Lire le TCB courant (GS:[0x20] = current_tcb dans PerCpuData).
    let tcb_ptr: u64;
    // SAFETY: segment GS initialisé avant tout traitement d'interruption.
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nostack, nomem),
        );
    }

    // Positionner NEED_RESCHED sur le thread courant (RÈGLE IPI-01 DOC3).
    // Le reschedule effectif aura lieu au retour d'interruption quand le code
    // kernel vérifiera le flag (ou à l'IRET vers Ring 3).
    // Si le scheduler n'est pas encore initialisé (tcb_ptr == 0), ignoré.
    // SAFETY: sched_ipi_reschedule() est thread-safe pour le CPU courant.
    unsafe { sched_ipi_reschedule(tcb_ptr as *mut u8); }

    // EOI Local APIC — acquitter l'IPI avant le retour d'interruption.
    // SAFETY: LAPIC initialisé avant tout IPI SMP.
    super::apic::eoi();
}

/// IPI TLB shootdown (0xF2)
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
#[no_mangle]
extern "C" fn do_ipi_tlb_shootdown(_frame: *mut ExceptionFrame) {
    super::idt::irq_counter_inc(0xF2);
    super::paging::inc_tlb_shootdown();

    // Mode ExoPhoenix : 0xF2 = snapshot PMC lock-free.
    if crate::exophoenix::stage0::exophoenix_vectors_active() {
        // SAFETY: handler dédié no-alloc/no-lock.
        unsafe { crate::exophoenix::interrupts::handle_pmc_snapshot_ipi() };
        return;
    }

    // Identifiant logique du CPU courant (0-based).
    let cpu_id = super::smp::percpu::current_cpu_id() as u8;

    // Exécuter le flush TLB demandé par l'émetteur du shootdown.
    // SAFETY: appelé depuis handler IRQ (cli implicite); TLB_QUEUE thread-safe par spinlock.
    unsafe {
        crate::memory::virt::address_space::tlb::TLB_QUEUE.handle_remote(cpu_id);
    }

    // EOI Local APIC — signale la fin du traitement de l'interruption.
    // SAFETY: LAPIC initialisé avant tout IPI ; EOI est idempotent à ce stade.
    super::apic::eoi();
}

/// IPI hotplug CPU (0xF3)
#[no_mangle]
extern "C" fn do_ipi_cpu_hotplug(_frame: *mut ExceptionFrame) {
    super::idt::irq_counter_inc(0xF3);

    // Mode ExoPhoenix : 0xF3 = TLB flush + ACK lock-free.
    if crate::exophoenix::stage0::exophoenix_vectors_active() {
        // SAFETY: handler dédié no-alloc/no-lock.
        unsafe { crate::exophoenix::interrupts::handle_tlb_flush_ipi() };
        return;
    }

    // EOI avant halt pour que le BSP ne reste pas bloqué en attente.
    // SAFETY: LAPIC initialisé avant tout IPI hotplug.
    super::apic::eoi();
    // Identifier ce CPU et le mettre hors ligne (→ ! — ce CPU ne revient pas).
    let cpu_id = super::smp::percpu::current_cpu_id();
    // SAFETY: hotplug_cpu_halt est idempotent et -> ! (arrêt irréversible).
    super::smp::hotplug::hotplug_cpu_halt(cpu_id);
}

/// IPI panic broadcast (0xFE)
///
/// Reçu par tous les APs lors d'un kernel panic.
/// Arrêt immédiat sans tentative de sauvegarde.
#[no_mangle]
extern "C" fn do_ipi_panic(_frame: *mut ExceptionFrame) {
    // SAFETY: réponse à un panic — arrêt irréversible requis
    super::halt_cpu();
}

// ── Kernel Panic ──────────────────────────────────────────────────────────────

/// Affiche les informations de l'exception et arrête le CPU
///
/// # RÈGLE NO-ALLOC
/// Cette fonction ne peut PAS allouer de mémoire.
/// Elle affiche uniquement les registres depuis la frame.
#[cold]
#[inline(never)]
fn kernel_panic_exception(msg: &str, frame: &ExceptionFrame) -> ! {
    // Dans une implémentation complète : écrire sur un port série ou VGA buffer.
    // Pour l'instant : consommer les paramètres pour éviter les warnings.
    let _ = msg;
    let _ = frame;

    // Broadcaster l'IPI panic vers tous les APs pour les arrêter.
    // SAFETY: situation non-récupérable ; broadcast_ipi_panic() n'alloue pas.
    super::apic::ipi::broadcast_ipi_panic();

    // SAFETY: situation non-récupérable — HLT en boucle.
    super::halt_cpu()
}

// ── Instrumentations ──────────────────────────────────────────────────────────

/// Compteurs per-vecteur (vecteurs 0–31 seulement ici)
static EXC_COUNTERS: [AtomicU64; 32] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 32]
};

static NMI_COUNT:                   AtomicU64 = AtomicU64::new(0);
static MC_COUNT:                    AtomicU64 = AtomicU64::new(0);
static GP_FAULT_COUNT:              AtomicU64 = AtomicU64::new(0);
static TIMER_IRQ_COUNT:             AtomicU64 = AtomicU64::new(0);
static SPURIOUS_IRQ_COUNT:          AtomicU64 = AtomicU64::new(0);
static FPU_DEVICE_NOT_AVAIL_COUNT:  AtomicU64 = AtomicU64::new(0);

pub fn exc_count(vector: u8)           -> u64 { if (vector as usize) < 32 { EXC_COUNTERS[vector as usize].load(Ordering::Relaxed) } else { 0 } }
pub fn nmi_count()                     -> u64 { NMI_COUNT.load(Ordering::Relaxed) }
pub fn machine_check_count()           -> u64 { MC_COUNT.load(Ordering::Relaxed) }
pub fn gp_fault_count()                -> u64 { GP_FAULT_COUNT.load(Ordering::Relaxed) }
pub fn timer_irq_count()               -> u64 { TIMER_IRQ_COUNT.load(Ordering::Relaxed) }
pub fn spurious_irq_count()            -> u64 { SPURIOUS_IRQ_COUNT.load(Ordering::Relaxed) }
pub fn fpu_device_not_avail_count()    -> u64 { FPU_DEVICE_NOT_AVAIL_COUNT.load(Ordering::Relaxed) }
