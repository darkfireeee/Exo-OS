//! # syscall/dispatch/signals.rs — Gestion et livraison des signaux pour le dispatch
//!
//! Orchestration des signaux au retour vers l'espace utilisateur :
//! - Restauration du contexte utilisateur pour `rt_sigreturn` (SIG-13 / SIG-14)
//! - Vérification et livraison des signaux pendants avant SYSRETQ (RÈGLE SIGNAL-01)

use crate::arch::x86_64::syscall::SyscallFrame;

// ─────────────────────────────────────────────────────────────────────────────
// Traitement spécial rt_sigreturn (SIG-13 / SIG-14)
// ─────────────────────────────────────────────────────────────────────────────

/// Traite `SYS_RT_SIGRETURN` directement depuis la frame arch.
///
/// Contrairement aux autres syscalls, `rt_sigreturn` doit modifier les registres
/// userspace (RIP, RSP, RFLAGS, RAX…) retournés par SYSRETQ. Le handler normal
/// n'a pas accès à la `SyscallFrame` — ce traitement spécial le permet.
///
/// ## Protocole (POSIX / SIG-13)
/// 1. RSP userspace au moment de SYSRETQ = sig_rsp + 8 (le `ret` du handler a
///    consommé `pretcode`).
/// 2. `sig_rsp = frame.user_rsp - 8`.
/// 3. UContext à `sig_rsp + SIGNAL_FRAME_UC_OFFSET`.
/// 4. Magic `0x5349_474E` vérifié constant-time (LAC-01 / SIG-14).
/// 5. Si magic OK : registres + signal_mask restaurés.
/// 6. RSP userspace mis à jour dans `gs:[0x08]` pour SYSRETQ.
///
/// En cas d'échec (magic invalide, adresse invalide) : le processus reçoit SIGSEGV
/// au retour vers userspace (RIP sera 0, qui est non-mappé).
/// Sanitise RFLAGS pour le retour en Ring 3 (anti-élévation IOPL/VM/TF, force IF=1).
#[inline]
pub fn sanitize_user_rflags(rflags: u64) -> u64 {
    const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0024_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
    const RFLAGS_FORCE_SET: u64 = 0x0000_0000_0000_0200; // IF=1 (interruptions activées en user)
    const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_7100; // TF=0, IOPL=0, NT=0, RF=0, VM=0
    ((rflags & RFLAGS_SAFE_MASK) | RFLAGS_FORCE_SET) & !RFLAGS_FORCE_CLR
}

pub fn handle_sigreturn_inplace(frame: &mut SyscallFrame) {
    use crate::process::signal::handler::{verify_and_extract_uc, SIGNAL_FRAME_UC_OFFSET};
    use crate::scheduler::core::task::ThreadControlBlock;

    // Le `ret` du handler a popped `pretcode` → RSP userspace = sig_rsp + 8.
    let sig_rsp = frame.rsp.wrapping_sub(8);
    let uc_ptr = sig_rsp + SIGNAL_FRAME_UC_OFFSET;

    let regs = match verify_and_extract_uc(uc_ptr) {
        Some(r) => r,
        None => {
            // Magic invalide ou adresse corrompue : déclencher SIGSEGV.
            // On met RIP = 0 (non-mappé) → #PF Ring3 → SIGSEGV au processus.
            // On ne crashe jamais en Ring0 (ARCH-SYSRET V-35 respecté).
            frame.rcx = 0;
            frame.rax = (-crate::syscall::errno::EFAULT) as u64;
            return;
        }
    };

    // Restaurer les registres dans la frame arch (lus par SYSRETQ).
    // RIP de retour = rcx pour SYSRETQ.
    frame.rcx = regs.rip;
    // RFLAGS userspace = r11 pour SYSRETQ avec masquage strict anti-élévation (T9-006 / T6-001).
    frame.r11 = sanitize_user_rflags(regs.rflags);
    frame.rax = regs.rax;
    frame.rdi = regs.rdi;
    frame.rsi = regs.rsi;
    frame.rdx = regs.rdx;
    frame.r8 = regs.r8;
    frame.r9 = regs.r9;
    frame.r10 = regs.r10;
    frame.r12 = regs.r12;
    frame.r13 = regs.r13;
    frame.r14 = regs.r14;
    frame.r15 = regs.r15;
    frame.rbx = regs.rbx;
    frame.rbp = regs.rbp;
    // RSP userspace mis à jour dans gs:[0x08] ET dans frame.rsp.
    // SAFETY: GS kernel actif, gs:[0x08] = user_rsp slot du PerCpuData.
    unsafe {
        core::arch::asm!(
            "mov qword ptr gs:[0x08], {rsp}",
            rsp = in(reg) regs.rsp,
            options(nostack, nomem)
        );
    }
    frame.rsp = regs.rsp;

    // Restaurer le masque de signal dans le TCB.
    // SAFETY: gs:[0x20] = pointeur TCB courant (PerCpuData layout).
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr != 0 {
        // Masque SIGKILL/SIGSTOP non-masquables et bits de signaux invalides.
        let safe_mask = crate::process::signal::mask::SigMask::from(regs.signal_mask).0;
        // SAFETY: tcb_ptr est non-nul et valide (maintenu par le scheduler).
        let tcb = unsafe { &mut *(tcb_ptr as *mut ThreadControlBlock) };
        tcb.signal_mask
            .store(safe_mask, core::sync::atomic::Ordering::Release);
        if crate::memory::core::address::is_user_canonical(regs.fs_base) {
            tcb.fs_base = regs.fs_base;
            #[cfg(target_os = "none")]
            unsafe {
                crate::arch::x86_64::cpu::msr::write_msr(
                    crate::arch::x86_64::cpu::msr::MSR_FS_BASE,
                    regs.fs_base,
                );
            }
        }
        if crate::memory::core::address::is_user_canonical(regs.gs_base) {
            tcb.user_gs_base = regs.gs_base;
            #[cfg(target_os = "none")]
            unsafe {
                crate::arch::x86_64::cpu::msr::write_msr(
                    crate::arch::x86_64::cpu::msr::MSR_KERNEL_GS_BASE,
                    regs.gs_base,
                );
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Vérification et livraison des signaux pendants
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie et livre les signaux pending avant le retour userspace.
///
/// ## RÈGLE SIGNAL-01 / SIGNAL-02 (DOC1)
/// - `arch/` orchestre (lit le flag, appelle process/signal/).
/// - `process/signal/delivery` livre effectivement.
/// - `scheduler/` ne livre jamais directement.
///
/// ## Implémentation
/// Lit `gs:[0x20]` → pointeur TCB → champ `signal_pending`.
/// Si posé → appelle `process::signal::delivery::handle_pending_signals()`.
/// La livraison modifie la frame si elle installe un handler userspace.
#[inline]
pub fn check_and_deliver_signals(frame: &mut SyscallFrame) {
    // SAFETY: GS kernel actif dans ce contexte (SWAPGS dans le stub ASM).
    // gs:[0x20] contient le pointeur TCB, potentiellement nul si pas encore
    // initialisé (cas du kernel avant le premier fork → aucun signal possible).
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x20]",
            out(reg) tcb_ptr,
            options(nostack, nomem)
        );
    }

    if tcb_ptr == 0 {
        // Aucun thread courant (boot / idle) → pas de signal.
        return;
    }

    // SAFETY: tcb_ptr est non-nul et pointe vers un ThreadControlBlock valide
    // (construit par lifecycle/create.rs et maintenu par le scheduler).
    use crate::scheduler::core::task::ThreadControlBlock;
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };

    // Vérifier le flag avec Acquire pour synchroniser avec l'écriture signal_pending
    // effectuée par process/signal/ (Ordering::Release s'y applique).
    if !tcb.has_signal_pending() {
        return;
    }

    // ── Conversion arch::SyscallFrame → delivery::SyscallFrame ───────────────
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::signal::delivery::{handle_pending_signals, SyscallFrame as DeliveryFrame};

    let pid = Pid(tcb.pid.0);
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return,
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        return;
    }

    // SAFETY: thread_ptr maintenu par pcb, valide dans ce contexte.
    let thread = unsafe { &mut *thread_ptr };

    let mut d_frame = DeliveryFrame {
        user_rip: frame.rcx,    // RIP de retour (sauvé par SYSCALL hw dans RCX)
        user_rflags: frame.r11, // RFLAGS (sauvé par SYSCALL hw dans R11)
        user_rsp: frame.rsp,    // RSP userspace
        user_rax: frame.rax,    // valeur de retour syscall
        user_rdi: frame.rdi,
        user_rsi: frame.rsi,
        user_rdx: frame.rdx,
        user_rcx: frame.rcx, // userspace RCX = même que RIP retour (SYSCALL)
        user_r8: frame.r8,
        user_r9: frame.r9,
        user_r10: frame.r10,
        user_r12: frame.r12,
        user_r13: frame.r13,
        user_r14: frame.r14,
        user_r15: frame.r15,
        user_rbx: frame.rbx,
        user_rbp: frame.rbp,
        user_fs_base: thread.sched_tcb.fs_base,
        user_gs_base: thread.sched_tcb.user_gs_base,
        user_cs: 0x1B, // CS ring3 (non sauvé par SYSCALL mais requis)
        user_ss: 0x23, // SS ring3
    };

    handle_pending_signals(thread, &mut d_frame);

    // Répercuter les modifications potentielles (ex. RIP redirigé, RSP vers sigaltstack).
    frame.rcx = d_frame.user_rip;
    frame.r11 = sanitize_user_rflags(d_frame.user_rflags);
    frame.rax = d_frame.user_rax;
    frame.rdi = d_frame.user_rdi;
    frame.rsi = d_frame.user_rsi;
    frame.rdx = d_frame.user_rdx;
    frame.r8 = d_frame.user_r8;
    frame.r9 = d_frame.user_r9;
    frame.r10 = d_frame.user_r10;
    frame.r12 = d_frame.user_r12;
    frame.r13 = d_frame.user_r13;
    frame.r14 = d_frame.user_r14;
    frame.r15 = d_frame.user_r15;
    frame.rbx = d_frame.user_rbx;
    frame.rbp = d_frame.user_rbp;

    // RSP userspace : mettre à jour gs:[0x08] si modifié (sigaltstack / setup_signal_frame).
    if d_frame.user_rsp != frame.rsp {
        let new_rsp = d_frame.user_rsp;
        // SAFETY: GS kernel actif, gs:[0x08] = user_rsp slot du PerCpuData.
        unsafe {
            core::arch::asm!(
                "mov qword ptr gs:[0x08], {rsp}",
                rsp = in(reg) new_rsp,
                options(nostack, nomem)
            );
        }
        frame.rsp = new_rsp;
    }
}
