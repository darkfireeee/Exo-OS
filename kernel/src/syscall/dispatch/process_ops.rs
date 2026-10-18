//! # syscall/dispatch/process_ops.rs — Traitements spéciaux pour fork, vfork et execve
//!
//! Ces appels système requièrent un accès direct et une modification de la `SyscallFrame`
//! matérielle (RIP de retour, RSP userspace, pointeurs d'arguments, rechargement CR3).

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use crate::arch::x86_64::syscall::SyscallFrame;
use super::stats::{exec_trace, syscall_trace};

// ─────────────────────────────────────────────────────────────────────────────
// Traitement spécial fork / vfork (SYS_FORK = 57, SYS_VFORK = 58)
// ─────────────────────────────────────────────────────────────────────────────

/// Traite `SYS_FORK` ou `SYS_VFORK` directement depuis la frame arch.
///
/// Fork a besoin de `frame.rcx` (RIP de retour = point de bifurcation) et
/// `frame.rsp` (RSP userspace). Ces valeurs ne transitent pas dans les arguments
/// syscall normaux (rdi..r9), donc le handler normal ne peut pas les lire.
///
/// ## Protocole (POSIX fork)
/// 1. Lit TCB courant via `gs:[0x20]` → PID → PCB.
/// 2. Récupère le `ProcessThread` via `pcb.main_thread_ptr()`.
/// 3. Construit `ForkContext` depuis la frame syscall complète.
/// 4. Appelle `do_fork()` — crée PCB/TCB fils, CoW, TLB flush, enqueue RunQueue.
/// 5. Retourne `child_pid` au parent (fils démarre via `fork_child_trampoline`).
pub fn handle_fork_like_inplace(
    frame: &SyscallFrame,
    fork_flags: crate::process::lifecycle::fork::ForkFlags,
) -> i64 {
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::lifecycle::fork::{
        do_fork, wait_for_vfork_completion, ForkContext, ForkError,
    };
    use crate::scheduler::core::task::ThreadControlBlock;
    use crate::syscall::errno::{EAGAIN, EFAULT, EINTR, EINVAL, ENOMEM};

    // Lire TCB courant.
    // SAFETY: GS kernel actif, gs:[0x20] = pointeur TCB (PerCpuData).
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        return EAGAIN;
    }

    // SAFETY: tcb_ptr non-nul et maintenu par le scheduler.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let pid = Pid(tcb.pid.0);

    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return EAGAIN,
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        return EAGAIN;
    }

    // SAFETY: thread_ptr maintenu par pcb, valide tant que pcb est dans PROCESS_REGISTRY.
    let thread = unsafe { &*thread_ptr };

    let ctx = ForkContext {
        parent_thread: thread,
        parent_pcb: pcb,
        flags: fork_flags,
        target_cpu: tcb.current_cpu().0,
        child_rip: frame.rcx,     // RIP de retour sauvé par SYSCALL hw
        child_rsp: frame.rsp,     // RSP userspace sauvé au stub ASM
        parent_rflags: frame.r11, // RFLAGS sauvés par SYSCALL hw — CORRECTION P2-02
        user_rbx: frame.rbx,
        user_rbp: frame.rbp,
        user_r12: frame.r12,
        user_r13: frame.r13,
        user_r14: frame.r14,
        user_r15: frame.r15,
        user_rdi: frame.rdi,
        user_rsi: frame.rsi,
        user_rdx: frame.rdx,
        user_r10: frame.r10,
        user_r8: frame.r8,
        user_r9: frame.r9,
    };

    match do_fork(&ctx) {
        Ok(result) => {
            syscall_trace(b"sys_fork: do_fork ok\n");
            if fork_flags.has(crate::process::lifecycle::fork::ForkFlags::VFORK) {
                // SAFETY: tcb_ptr pointe le TCB courant; `do_fork` est terminé et
                // aucune référence immutable au TCB n'est réutilisée après ce point.
                let tcb_mut = unsafe { &mut *(tcb_ptr as *mut ThreadControlBlock) };
                if wait_for_vfork_completion(result.child_pid, tcb_mut).is_err() {
                    return EINTR;
                }
            }
            result.child_pid.0 as i64
        }
        Err(e) => {
            syscall_trace(b"sys_fork: do_fork err\n");
            match e {
                ForkError::PidExhausted
                | ForkError::TidExhausted
                | ForkError::RegistryError
                | ForkError::InvalidCpu => EAGAIN,
                ForkError::OutOfMemory | ForkError::AddressSpaceCloneFailed => ENOMEM,
                ForkError::NoAddrCloner => EFAULT,
                ForkError::UnsupportedFlag => EINVAL,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Traitement spécial execve (SYS_EXECVE = 59)
// ─────────────────────────────────────────────────────────────────────────────

/// Copie un tableau de chaînes null-terminé depuis l'espace utilisateur.
///
/// `argv_ptr` pointe vers un tableau de pointeurs `char*` terminé par NULL.
/// Retourne `Some(Vec<String>)` ou `None` si une adresse est invalide.
///
/// EXEC-01 : seule fonction autorisée à lire argv/envp depuis userspace.
pub fn copy_userspace_argv(
    argv_ptr: u64,
    max_args: usize,
) -> Option<Vec<String>> {
    use crate::syscall::validation::{copy_from_user, UserStr, USER_ADDR_MAX};

    if argv_ptr == 0 {
        return Some(Vec::new());
    }
    if argv_ptr >= USER_ADDR_MAX {
        return None;
    }

    let mut result: Vec<String> = Vec::new();
    let mut saw_null = false;

    for i in 0..max_args {
        // Lire le i-ème pointeur (u64) du tableau.
        let ptr_addr = match argv_ptr.checked_add(i as u64 * 8) {
            Some(a) if a <= USER_ADDR_MAX.saturating_sub(8) => a,
            _ => return None,
        };

        let mut raw = [0u8; core::mem::size_of::<u64>()];
        copy_from_user(raw.as_mut_ptr(), ptr_addr as *const u8, raw.len()).ok()?;
        let str_ptr = u64::from_ne_bytes(raw);

        if str_ptr == 0 {
            saw_null = true;
            break;
        } // terminateur NULL du tableau
        if str_ptr >= USER_ADDR_MAX {
            return None;
        }

        let user_str = UserStr::from_user(str_ptr, 4096).ok()?;

        // Conversion UTF-8 permissive (remplace les octets invalides par U+FFFD).
        result.push(String::from_utf8_lossy(user_str.as_bytes()).into_owned());
    }

    if saw_null {
        Some(result)
    } else {
        None
    }
}

/// Traite `SYS_EXECVE` directement depuis la frame arch.
///
/// En cas de succès, `do_execve()` remplace l'image du processus mais retourne
/// normalement en Rust. Ce handler met alors à jour la frame arch pour que
/// SYSRETQ saute directement au nouveau point d'entrée ELF.
///
/// ## Protocole
/// 1. Lit path depuis userspace via `frame.rdi`.
/// 2. Récupère TCB → PCB → ProcessThread.
/// 3. Appelle `do_execve()`.
/// 4. Succès : `frame.rcx = entry_point`, `frame.rsp = initial_rsp`, `frame.rax = 0`.
/// 5. Échec : `frame.rax = -errno`.
///
/// ## Note : argv/envp
/// Les tableaux `argv` et `envp` sont copiés avant le remplacement d'image via
/// les primitives userspace centralisées (`copy_from_user` / `UserStr`).
pub fn handle_execve_inplace(frame: &mut SyscallFrame) {
    use crate::process::core::pid::Pid;
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::lifecycle::exec::{do_execve, ElfLoadError, ExecError};
    use crate::scheduler::core::task::ThreadControlBlock;
    use crate::syscall::errno::*;
    use crate::syscall::validation::read_user_path;

    exec_trace(b"execve: enter\n");

    // Lire le chemin depuis userspace.
    let user_path = match read_user_path(frame.rdi) {
        Ok(p) => p,
        Err(e) => {
            exec_trace(b"execve: path fault\n");
            frame.rax = e.to_errno() as u64;
            return;
        }
    };
    let path = match user_path.as_str() {
        Ok(s) => s,
        Err(_) => {
            exec_trace(b"execve: path utf8\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };
    // DIAG E9: print étiqueté du chemin execve (non-ambigu, greppable).
    crate::arch::x86_64::terminal::debug_write(b"\n<EXEC ");
    crate::arch::x86_64::terminal::debug_write(path.as_bytes());
    crate::arch::x86_64::terminal::debug_write(b">");

    // Lire TCB courant.
    // SAFETY: GS kernel actif.
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        exec_trace(b"execve: no tcb\n");
        frame.rax = EFAULT as u64;
        return;
    }

    // SAFETY: tcb_ptr maintenu par le scheduler.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let pid = Pid(tcb.pid.0);

    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => {
            exec_trace(b"execve: no pcb\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };

    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        exec_trace(b"execve: no thread\n");
        frame.rax = EAGAIN as u64;
        return;
    }

    // SAFETY: thread_ptr maintenu par pcb.
    let thread = unsafe { &mut *thread_ptr };

    // ARGV-01 (EXEC-01) : copier argv/envp depuis userspace avant do_execve().
    // frame.rsi = argv_ptr (tableau de char* null-terminé)
    // frame.rdx = envp_ptr (tableau de char* null-terminé)
    let argv_strings = match copy_userspace_argv(frame.rsi, 1024) {
        Some(v) => v,
        None => {
            exec_trace(b"execve: argv fault\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();

    let envp_strings = match copy_userspace_argv(frame.rdx, 4096) {
        Some(v) => v,
        None => {
            exec_trace(b"execve: env fault\n");
            frame.rax = EFAULT as u64;
            return;
        }
    };
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();

    exec_trace(b"execve: load\n");
    match do_execve(thread, pcb, &path, &argv_refs, &envp_refs) {
        Ok(()) => {
            exec_trace(b"execve: ok\n");
            crate::arch::x86_64::terminal::debug_write(b"=OK\n");
            // Succès : lire le nouveau point d'entrée depuis le ProcessThread mis à jour.
            let new_rip = thread.addresses.entry_point;
            let new_rsp = thread.addresses.initial_rsp;

            // Mettre à jour la frame pour SYSRETQ.
            frame.rbx = 0;
            frame.rbp = 0;
            frame.r12 = 0;
            frame.r13 = 0;
            frame.r14 = 0;
            frame.r15 = 0;
            frame.rdi = thread.addresses.entry_arg0;
            frame.rcx = new_rip; // RIP → nouveau point d'entrée ELF
            frame.rsp = new_rsp; // RSP → nouvelle pile userspace
            frame.r11 = 0x0202; // RFLAGS : IF=1, bit réservé=1
            frame.rax = 0; // "succès" (non retourné — SYSRETQ saute)

            // SYSRETQ lit gs:[0x08] pour restaurer le RSP userspace (stub ASM).
            // SAFETY: GS kernel actif, gs:[0x08] = user_rsp slot du PerCpuData.
            unsafe {
                core::arch::asm!(
                    "mov qword ptr gs:[0x08], {rsp}",
                    rsp = in(reg) new_rsp,
                    options(nostack, nomem)
                );
            }

            // FIX-EXEC-CR3 : execve remplace l'espace d'adressage SANS context
            // switch. Le stub de retour syscall (syscall.rs) recharge CR3 depuis
            // le slot per-CPU gs:[0x48] (user CR3), qui n'est mis à jour que par
            // `set_current_cr3()` lors d'un context switch. Sans cette
            // republication, SYSRETQ revient avec l'ANCIEN CR3 : le processus
            // reprend au nouveau RIP d'entrée mais dans l'ancien espace
            // d'adressage (init et les serveurs sont tous liés à la même base
            // virtuelle), exécutant le code de l'ancienne image → boucle
            // fork/execve sans jamais charger la nouvelle image.
            // DIAG E9: 'K' = chemin fix CR3 execve exécuté.
            unsafe { core::arch::asm!("out 0xE9, al", in("al") b'K', options(nomem, nostack)) };
            let new_kernel_cr3 = thread.sched_tcb.cr3_phys;
            let new_user_cr3 = thread.sched_tcb.kpti_user_cr3();
            if crate::arch::x86_64::spectre::kpti::kpti_enabled() {
                crate::arch::x86_64::spectre::kpti::set_current_cr3(new_kernel_cr3, new_user_cr3);
            } else {
                // KPTI désactivé : gs:[0x40] et gs:[0x48] DOIVENT rester à 0.
                // Appeler set_current_cr3() avec KPTI off écrivait new_kernel_cr3
                // dans gs:[0x40] et 0 dans gs:[0x48] ; comme context_switch() ne
                // met pas à jour gs:[0x40] quand KPTI est off, toute entrée
                // syscall/IRQ/exception ultérieure d'un autre processus (PID 1)
                // rechargeait le CR3 de cet execve depuis gs:[0x40] et revenait
                // en Ring 3 (gs:[0x48]==0) sur le mauvais espace d'adressage (#25).
                crate::arch::x86_64::smp::percpu::set_kpti_cr3_slots(0, 0);
                // SAFETY: new_kernel_cr3 = PML4 de la nouvelle image, moitié
                // noyau synchronisée par do_execve(); Ring 0.
                unsafe {
                    core::arch::asm!("mov cr3, {}", in(reg) new_kernel_cr3, options(nostack, nomem));
                }
            }
        }
        Err(e) => {
            exec_trace(b"execve: err\n");
            crate::arch::x86_64::terminal::debug_write(match e {
                ExecError::ElfLoadFailed(ElfLoadError::NotFound) => b"=ER:NotFound\n" as &[u8],
                ExecError::ElfLoadFailed(ElfLoadError::InvalidElf) => b"=ER:InvalidElf\n",
                ExecError::ElfLoadFailed(ElfLoadError::PermissionDenied) => b"=ER:Perm\n",
                ExecError::ElfLoadFailed(ElfLoadError::OutOfMemory) => b"=ER:ElfOOM\n",
                ExecError::ElfLoadFailed(ElfLoadError::UnsupportedArch) => b"=ER:Arch\n",
                ExecError::ElfLoadFailed(ElfLoadError::InterpreterNotFound) => b"=ER:NoInterp\n",
                ExecError::OutOfMemory => b"=ER:OOM\n",
                ExecError::ThreadGroupNotSingle => b"=ER:NotSingle\n",
                ExecError::NoLoader => b"=ER:NoLoader\n",
                _ => b"=ER:Other\n",
            });
            let errno: i64 = match e {
                ExecError::ElfLoadFailed(ElfLoadError::NotFound) => ENOENT,
                ExecError::ElfLoadFailed(ElfLoadError::PermissionDenied)
                | ExecError::PermissionDenied => EACCES,
                ExecError::ElfLoadFailed(ElfLoadError::OutOfMemory) => ENOMEM,
                ExecError::ElfLoadFailed(ElfLoadError::InvalidElf) => EINVAL,
                ExecError::ArgListTooLong => E2BIG,
                ExecError::NameTooLong => EINVAL,
                ExecError::OutOfMemory => ENOMEM,
                ExecError::ThreadGroupNotSingle => EBUSY,
                ExecError::NoLoader => ENOSYS,
                _ => ENOSYS,
            };
            frame.rax = errno as u64;
        }
    }
}
