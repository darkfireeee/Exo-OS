#![allow(unused_imports, dead_code)]
use super::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::x86_64::{apic, halt_cpu, idt, memory_iface, paging, read_cr2, read_cr3, smp};

// ── Ponts C ABI vers le scheduler (RÈGLE FPU-02 + RÈGLE IPI-01 DOC3) ─────────
//
// arch/ ne peut pas importer scheduler/ directement (éviter cycle de dépendance
// Rust crate-level). Les deux fonctions ci-dessous sont exportées par le
// scheduler via `#[no_mangle] pub unsafe extern "C"` et résolues à l'édition
// des liens.
//
// sched_fpu_handle_nm   — scheduler/fpu/lazy.rs     — handler #NM (FPU lazy)
// sched_ipi_reschedule  — scheduler/timer/tick.rs   — IPI reschedule
extern "C" {
    /// Gère l'exception #NM (Device Not Available) pour le TCB courant.
    /// Efface CR0.TS et restaure le contexte FPU via xrstor.
    /// `tcb_ptr` = GS:[0x20] (current_tcb). Si null → simple clts.
    pub fn sched_fpu_handle_nm(tcb_ptr: *mut u8);
    /// Positionne NEED_RESCHED sur le thread courant suite à un IPI reschedule.
    /// `tcb_ptr` = GS:[0x20] (current_tcb). Si null → ignoré.
    pub fn sched_ipi_reschedule(tcb_ptr: *mut u8);
    /// Tick du scheduler timer : avance les quantum, décide préemption.
    /// `cpu_id` = GS:[0x10], `current` = GS:[0x20].
    pub fn scheduler_tick(cpu_id: u32, current: *mut u8);
    /// Pont arch/→process/ : livraison signaux au retour exception Ring 3.
    /// `tcb_ptr` = GS:[0x20], `excframe` = &mut ExceptionFrame courant.
    pub fn proc_signal_on_exception_return(tcb_ptr: *mut u8, excframe: *mut u8);
}

// ── Frame d'exception ─────────────────────────────────────────────────────────

/// Registres sauvegardés par le CPU + l'ASM de stub lors d'une exception
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ExceptionFrame {
    // Registres sauvegardés par l'ASM stub (caller-saved)
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // Poussés par le stub ou le CPU selon l'exception
    pub error_code: u64,

    // Poussés automatiquement par le CPU
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64, // Présent uniquement si changement de niveau de privilège
    pub ss: u64,  // Idem
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


// ── Handlers Rust d'exceptions ────────────────────────────────────────────────

#[inline(always)]
pub fn current_tcb_raw_checked() -> u64 {
    // SAFETY: la lecture vérifie d'abord que MSR_GS_BASE pointe sur la zone
    // per-CPU kernel avant de toucher `gs:[0x20]`.
    unsafe { crate::arch::x86_64::smp::percpu::try_read_current_tcb().unwrap_or(0) }
}

#[inline(always)]
pub fn current_cpu_id_checked() -> u32 {
    crate::arch::x86_64::smp::percpu::try_current_cpu_id().unwrap_or(0)
}

/// Retour vers userspace après exception depuis Ring 3
///
/// ## RÈGLE SIGNAL-01 (DOC1)
/// Point d'orchestration des signaux après préemption ou exception.
/// Vérifie `signal_pending` dans le TCB et orchestre la livraison.
/// La livraison effective est déléguée à `process::signal::delivery`.
pub fn exception_return_to_user_inner(frame: &mut ExceptionFrame, allow_resched: bool) {
    let tcb_ptr = current_tcb_raw_checked();
    if tcb_ptr == 0 {
        return;
    }
    // SAFETY: proc_signal_on_exception_return thread-safe; exclu par cli implicite dans handler.
    unsafe {
        proc_signal_on_exception_return(
            tcb_ptr as *mut u8,
            frame as *mut ExceptionFrame as *mut u8,
        );
    }

    if allow_resched {
        // Les IRQ timer/IPI ne font que poser NEED_RESCHED. Le retour Ring3 est le
        // point stable où l'on peut basculer vers un autre thread, en conservant la
        // frame d'exception sur la pile kernel du thread préempté.
        // SAFETY: appelé au retour d'IRQ, point de préemption stable ; la frame
        // d'exception reste sur la pile kernel du thread préempté pendant le switch.
        unsafe {
            let _ = crate::scheduler::core::switch::schedule_current_if_needed();
        }
    }
}

/// Retour normal depuis une exception Ring 3.
#[inline]
pub fn exception_return_to_user(frame: &mut ExceptionFrame) {
    exception_return_to_user_inner(frame, true);
}

/// Retourne d'une faute récupérée sans commuter de thread entre la réparation
/// de PTE et le rejeu de l'instruction fautive.
///
/// Une écriture de pile CoW peut avoir fauté au milieu d'un `call`. Replanifier
/// ici laisse l'enfant progresser pendant que le parent possède encore la frame
/// d'exception qui doit rejouer ce `call`; le retour d'adresse restait alors nul
/// dans le dump #25. Les signaux continuent à être délivrés, seule la préemption
/// est différée jusqu'au prochain point d'entrée noyau sûr.
#[inline]
pub fn exception_return_to_user_after_recovered_fault(frame: &mut ExceptionFrame) {
    exception_return_to_user_inner(frame, false);
}

#[inline]
pub fn queue_signal_for_current(sig: crate::process::signal::Signal) {
    let tcb_raw = current_tcb_raw_checked();
    // DIAG-V14-SIGTRACE (bisect #GP1 livelock, run 13/14) : le #GP1 identique
    // bit-pour-bit d'un run à l'autre, sans jamais un seul <SG pid=... k=...>
    // (imprimé par deliver_one avant toute action), prouve que la livraison
    if tcb_raw == 0 {
        return;
    }
    // SAFETY: tcb_raw != 0 vérifié ; pointe vers le TCB du thread courant publié
    // par le scheduler pour ce CPU, vivant pendant le handler.
    let tcb = unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
    let pid = crate::process::core::pid::Pid(tcb.pid.0);
    let _ = crate::process::signal::delivery::send_signal_to_pid(pid, sig);
}

/// DIAG-UD1/GP1 (bisect mort d'init) : dump complet d'une exception fatale
/// userspace — même format que le marqueur <SEGV> DIAG-25 de do_page_fault :
/// pid, rip, rsp, ripb (8 octets IN-MÉMOIRE à RIP : `0f 0b` en tête = vrai
/// `ud2` compilé ; octets aléatoires = saut indirect corrompu #25), registres
/// clés et 8 mots de pile user (chaîne de return addresses).
///
/// Lectures volatiles guardées : uniquement si l'adresse est une plage user
/// plausible — une lecture non gardée re-fauterait en kernel et détruirait le
/// dump.
pub fn diag_user_exception_dump(tag: &[u8], frame: &ExceptionFrame) {
    let out = crate::arch::x86_64::terminal::debug_write;
    let hex = |val: u64| {
        let lut = b"0123456789abcdef";
        let mut hb = [0u8; 16];
        let mut x = val;
        let mut k = 16;
        while k > 0 {
            k -= 1;
            hb[k] = lut[(x & 0xf) as usize];
            x >>= 4;
        }
        hb
    };

    out(tag);
    // PID décimal du thread courant.
    let tcb_raw = current_tcb_raw_checked();
    let mut v = if tcb_raw != 0 {
        // SAFETY: tcb_raw != 0 vérifié ; TCB courant publié par le scheduler.
        unsafe {
            (*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock))
                .pid
                .0 as u64
        }
    } else {
        0
    };
    if v == 0 {
        out(b"0");
    } else {
        let mut d = [0u8; 20];
        let mut i = d.len();
        while v != 0 && i > 0 {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        out(&d[i..]);
    }

    out(b" rip=");
    out(&hex(frame.rip));
    out(b" rsp=");
    out(&hex(frame.rsp));
    // Octets à RIP : lecture protégée par fixup pour éviter un #PF nested (T1-025 FIX).
    let mut rb = 0u64;
    let cpu = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;
    if frame.rip >= 0x1000 && frame.rip < 0x0000_8000_0000_0000 {
        let mut buf = [0u8; 8];
        if crate::syscall::fixup::copy_from_user_safe(buf.as_mut_ptr(), frame.rip as *const u8, 8, cpu) {
            rb = u64::from_le_bytes(buf);
        }
    }
    out(b" ripb=");
    out(&hex(rb));
    out(b" rax=");
    out(&hex(frame.rax));
    out(b" rcx=");
    out(&hex(frame.rcx));
    out(b" rdx=");
    out(&hex(frame.rdx));
    out(b" rsi=");
    out(&hex(frame.rsi));
    out(b" rdi=");
    out(&hex(frame.rdi));
    out(b" rbp=");
    out(&hex(frame.rbp));
    // DIAG-V8 (SEGV run 9) : r8-r15 étaient absents du dump — le registre
    // BASE de l'instruction fautive ([rbx-0x82]/[rbp-0x82]/[r8..15+disp]) est
    // probablement parmi eux. Sans eux, impossible de trancher entre « retour
    // syscall -11 déréférencé » et « pointeur rechargé d'une pile zéroée ».
    out(b" r8=");
    out(&hex(frame.r8));
    out(b" r9=");
    out(&hex(frame.r9));
    out(b" r10=");
    out(&hex(frame.r10));
    out(b" r11=");
    out(&hex(frame.r11));
    out(b" r12=");
    out(&hex(frame.r12));
    out(b" r13=");
    out(&hex(frame.r13));
    out(b" r14=");
    out(&hex(frame.r14));
    out(b" r15=");
    out(&hex(frame.r15));
    // 8 mots de pile user à RSP : la chaîne de return addresses — protégée par fixup (T1-025 FIX).
    out(b" stk=");
    if frame.rsp >= 0x1000 && frame.rsp < 0x0000_8000_0000_0000 {
        let mut words = [0u64; 8];
        if crate::syscall::fixup::copy_from_user_safe(words.as_mut_ptr() as *mut u8, frame.rsp as *const u8, 64, cpu) {
            for &w in &words {
                out(&hex(w));
                out(b" ");
            }
        }
    }
    // DIAG-UD1/SEGV étendu (run 5) : 16 octets à RDI — protégés par fixup (T1-025 FIX).
    out(b" rdib=");
    if frame.rdi >= 0x1000 && frame.rdi < 0x0000_8000_0000_0000 {
        let mut bytes = [0u8; 16];
        if crate::syscall::fixup::copy_from_user_safe(bytes.as_mut_ptr(), frame.rdi as *const u8, 16, cpu) {
            let lut = b"0123456789abcdef";
            for &byte in &bytes {
                out(&[lut[(byte >> 4) as usize], lut[(byte & 0xf) as usize]]);
            }
        }
    }
    out(b">\n");
}

#[cfg(all(debug_assertions, exo_kernel_trace))]
pub fn debug_hex64(value: u64) {
    let mut buf = [0u8; 16];
    let mut shift = 60u32;
    for byte in &mut buf {
        let nibble = ((value >> shift) & 0x0f) as u8;
        *byte = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        };
        shift = shift.saturating_sub(4);
    }
    crate::arch::x86_64::terminal::debug_write(&buf);
}

#[cfg(all(debug_assertions, exo_kernel_trace))]
pub fn debug_read_cr3() -> u64 {
    let cr3: u64;
    // SAFETY: lecture de CR3 en Ring 0, instruction sans effet mémoire.
    unsafe {
        core::arch::asm!(
            "mov {v}, cr3",
            v = out(reg) cr3,
            options(nostack, nomem, preserves_flags),
        );
    }
    cr3 & !0xFFFu64
}


// ── Kernel Panic ──────────────────────────────────────────────────────────────

/// Affiche les informations de l'exception et arrête le CPU
///
/// # RÈGLE NO-ALLOC
/// Cette fonction ne peut PAS allouer de mémoire.
/// Elle affiche uniquement les registres depuis la frame.
#[cold]
#[inline(never)]
pub fn kernel_panic_exception(msg: &str, frame: &mut ExceptionFrame) {
    if crate::exophoenix::resurrection::try_recover_exception(msg, frame) {
        return;
    }

    // Dans une implémentation complète : écrire sur un port série ou VGA buffer.
    // Pour l'instant : consommer les paramètres pour éviter les warnings.
    let _ = msg;
    let _ = frame;

    // Broadcaster l'IPI panic vers tous les APs pour les arrêter.
    // SAFETY: situation non-récupérable ; broadcast_ipi_panic() n'alloue pas.
    crate::arch::x86_64::apic::ipi::broadcast_ipi_panic();

    // SAFETY: situation non-récupérable — HLT en boucle.
    crate::arch::x86_64::halt_cpu()
}

// ── Instrumentations ──────────────────────────────────────────────────────────

/// Compteurs per-vecteur (vecteurs 0–31 seulement ici)
pub static EXC_COUNTERS: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

pub static NMI_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MC_COUNT: AtomicU64 = AtomicU64::new(0);
pub static GP_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static TIMER_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
pub static SPURIOUS_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
pub static FPU_DEVICE_NOT_AVAIL_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn exc_count(vector: u8) -> u64 {
    if (vector as usize) < 32 {
        EXC_COUNTERS[vector as usize].load(Ordering::Relaxed)
    } else {
        0
    }
}
pub fn nmi_count() -> u64 {
    NMI_COUNT.load(Ordering::Relaxed)
}
pub fn machine_check_count() -> u64 {
    MC_COUNT.load(Ordering::Relaxed)
}
pub fn gp_fault_count() -> u64 {
    GP_FAULT_COUNT.load(Ordering::Relaxed)
}
pub fn timer_irq_count() -> u64 {
    TIMER_IRQ_COUNT.load(Ordering::Relaxed)
}
pub fn spurious_irq_count() -> u64 {
    SPURIOUS_IRQ_COUNT.load(Ordering::Relaxed)
}
pub fn fpu_device_not_avail_count() -> u64 {
    FPU_DEVICE_NOT_AVAIL_COUNT.load(Ordering::Relaxed)
}

pub static USER_PF_DEBUG_COUNT: AtomicU64 = AtomicU64::new(0);
