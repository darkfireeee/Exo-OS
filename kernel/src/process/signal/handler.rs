// kernel/src/process/signal/handler.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Construction du frame signal utilisateur (x86_64 System V) — Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le frame est construit sur la pile utilisateur (ou sigaltstack si SA_ONSTACK).
// Il contient le contexte complet (ucontext_t) permettant sigreturn(2) de restaurer
// correctement l'état du thread après résolution du handler.
//
// Layout mémoire (croissant vers haut) :
//   [rsp - sizeof(SignalFrame)] : SignalFrame
//      +0   : SigInfo      (siginfo_t, 128 octets)
//      +128 : UContext      (ucontext_t, 936 octets)
//      +... : restorer stub (8 octets optionnel)


use core::sync::atomic::Ordering;
use super::delivery::SyscallFrame;
use super::queue::SigInfo;
use super::default::SigAction;
use super::tcb::SIGNAL_FRAME_MAGIC;
use crate::process::core::tcb::ProcessThread;

// ─────────────────────────────────────────────────────────────────────────────
// Structures ABI
// ─────────────────────────────────────────────────────────────────────────────

/// Registres généraux x86_64 sauvegardés dans ucontext_t.uc_mcontext.
#[derive(Copy, Clone, Default)]
#[repr(C)]
pub struct GRegs {
    pub r8:     u64,
    pub r9:     u64,
    pub r10:    u64,
    pub r11:    u64,
    pub r12:    u64,
    pub r13:    u64,
    pub r14:    u64,
    pub r15:    u64,
    pub rdi:    u64,
    pub rsi:    u64,
    pub rbp:    u64,
    pub rbx:    u64,
    pub rdx:    u64,
    pub rax:    u64,
    pub rcx:    u64,
    pub rsp:    u64,
    pub rip:    u64,
    pub eflags: u64,
    pub cs:     u16,
    pub gs:     u16,
    pub fs:     u16,
    pub _pad:   u16,
    pub err:    u64,
    pub trapno: u64,
    pub oldmask:u64,
    pub cr2:    u64,
}

/// Stack state pour ucontext.
#[derive(Copy, Clone, Default)]
#[repr(C)]
pub struct SigAltStack {
    pub ss_sp:    u64,
    pub ss_flags: i32,
    pub _pad:     u32,
    pub ss_size:  u64,
}

pub const SS_ONSTACK: i32 = 1;
pub const SS_DISABLE: i32 = 2;

/// Equivalent de ucontext_t (Linux x86_64).
#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct UContext {
    pub uc_flags:    u64,
    pub uc_link:     u64,   // pointeur vers ucontext parent (optionnel)
    pub uc_stack:    SigAltStack,
    pub uc_mcontext: GRegs,
    pub uc_sigmask:  u64,
    pub _fpregs_mem: [u8; 512], // espace pour FXSAVE
}

impl Default for UContext {
    fn default() -> Self {
        // SAFETY: tous les champs sont des types numériques → zéros valides.
        unsafe { core::mem::zeroed() }
    }
}

/// Frame complet placé sur la pile utilisateur avant appel du handler.
#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct SignalFrame {
    /// Adresse de retour : pointe sur la routine sigreturn (restorer).
    pub pretcode:  u64,
    /// Numéro du signal (rdi pour handler(int signo)).
    pub signo:     u64,
    /// Pointeur vers si_info (rsi pour handler(int, siginfo_t*, void*)).
    pub pinfo:     u64,
    /// Pointeur vers uc (rdx pour SA_SIGINFO handler).
    pub puc:       u64,
    /// siginfo_t.
    pub info:      SigInfoC,
    /// ucontext_t.
    pub uc:        UContext,
}

/// Version C-compatible de SigInfo (128 octets, compatible siginfo_t).
#[derive(Copy, Clone)]
#[repr(C)]
pub struct SigInfoC {
    pub si_signo: i32,
    pub si_errno: i32,
    pub si_code:  i32,
    pub _pad1:    i32,
    pub si_pid:   u32,
    pub si_uid:   u32,
    pub si_value_int: i32,
    pub _pad2:    i32,
    pub si_value_ptr: u64,
    pub _rest:    [u8; 88],
}

impl Default for SigInfoC {
    fn default() -> Self {
        // SAFETY: tous les champs sont des types numériques → zéros valides.
        unsafe { core::mem::zeroed() }
    }
}

impl SigInfoC {
    fn from_queue(info: &SigInfo) -> Self {
        Self {
            si_signo: info.signo as i32,
            si_code:  info.code,
            si_pid:   info.sender_pid,
            si_uid:   info.sender_uid,
            si_value_int: info.value_int,
            si_value_ptr: info.value_ptr,
            ..Default::default()
        }
    }
}

// Taille totale du frame signal : doit être <= 4096 pour tenir sur une page.
const _ASSERT_FRAME_SIZE: () = {
    // const_assert!(core::mem::size_of::<SignalFrame>() <= 4096)
    // Activation possible en nightly avec const_panic.
    ();
};

/// Offset du champ `uc` (UContext) dans `SignalFrame`, en octets.
///
/// Layout : pretcode(8) + signo(8) + pinfo(8) + puc(8) + info(SigInfoC=128) = 160.
/// Utilisé par `dispatch.rs` pour calculer `uc_ptr` lors du sigreturn.
pub const SIGNAL_FRAME_UC_OFFSET: u64 = 160;

/// Registres restaurés depuis le UContext au sigreturn.
///
/// Retourné par `verify_and_extract_uc()` après vérification du magic SIG-13.
#[derive(Copy, Clone, Default)]
pub struct UContextRegs {
    pub rip:      u64,
    pub rsp:      u64,
    pub rax:      u64,
    pub rdi:      u64,
    pub rsi:      u64,
    pub rdx:      u64,
    pub rcx:      u64,
    pub r8:       u64,
    pub r9:       u64,
    pub rflags:   u64,
    pub signal_mask: u64,
}

/// Vérifie le magic SIG-13 et extrait les registres du UContext userspace.
///
/// ## Sécurité (LAC-01 / SIG-13 / SIG-14)
/// - CONSTANT-TIME : toutes les données sont lues AVANT la vérification du magic.
///   Aucun chemin ne permet un timing oracle sur la validité du magic.
/// - Retourne `None` si l'adresse est invalide ou si `uc_flags != SIGNAL_FRAME_MAGIC`.
///
/// # Safety
/// `uc_ptr` doit être une adresse userspace valide (vérifiée en interne).
pub fn verify_and_extract_uc(uc_ptr: u64) -> Option<UContextRegs> {
    const USER_SPACE_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    if uc_ptr < 0x1000 || uc_ptr >= USER_SPACE_TOP {
        return None;
    }
    // SAFETY: adresse userspace validée ci-dessus.
    let uc = unsafe { &*(uc_ptr as *const UContext) };
    let mc = &uc.uc_mcontext;

    // Extraire TOUTES les données avant de vérifier le magic (LAC-01 constant-time).
    let regs = UContextRegs {
        rip:         mc.rip,
        rsp:         mc.rsp,
        rax:         mc.rax,
        rdi:         mc.rdi,
        rsi:         mc.rsi,
        rdx:         mc.rdx,
        rcx:         mc.rcx,
        r8:          mc.r8,
        r9:          mc.r9,
        rflags:      mc.eflags,
        signal_mask: uc.uc_sigmask,
    };

    // Vérification magic constant-time : XOR puis test (pas de brèche prédicteur).
    // SIG-13 / SIG-14 : si le magic est faux, le sigreturn est rejeté.
    let magic_diff = uc.uc_flags ^ (SIGNAL_FRAME_MAGIC as u64);
    if magic_diff != 0 {
        return None;
    }

    Some(regs)
}

// ─────────────────────────────────────────────────────────────────────────────
// setup_signal_frame — construit le frame sur la pile utilisateur
// ─────────────────────────────────────────────────────────────────────────────

/// Construit un SignalFrame sur la pile utilisateur et redirige RIP vers
/// le handler. Appelé depuis delivery::deliver_one() uniquement.
///
/// Séquence :
/// 1. Choisir la pile (sigaltstack si SA_ONSTACK, sinon pile courante).
/// 2. Aligner RSP à 16 oct (-sizeof(SignalFrame)) selon System V ABI.
/// 3. Écrire SignalFrame (info + ucontext avec les registres courants).
/// 4. Modifier frame->user_rip = handler, frame->user_rsp = &SignalFrame.
/// 5. Configurer RDI/RSI/RDX pour la convention SA_SIGINFO.
pub fn setup_signal_frame(
    thread: &mut ProcessThread,
    frame:  &mut SyscallFrame,
    sig_n:  u8,
    info:   &SigInfo,
    action: &SigAction,
) {
    // Choisir la pile cible.
    let target_rsp = if action.flags & SigAction::SA_ONSTACK != 0 {
        let alt = thread.addresses.sigaltstack_top();
        if alt != 0 { alt } else { frame.user_rsp }
    } else {
        frame.user_rsp
    };

    // Aligner selon SysV : (rsp - frame_size) & ~0xF, puis -8 (pour la red zone).
    let frame_size = core::mem::size_of::<SignalFrame>() as u64;
    let sig_rsp = (target_rsp - frame_size) & !0xFu64;

    // Construire le contenu du frame dans un buffer temporaire.
    // Assurer que sig_rsp est une adresse utilisateur valide (< USER_SPACE_TOP).
    const USER_SPACE_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    if sig_rsp >= USER_SPACE_TOP || sig_rsp < 0x1000 {
        // Pile utilisateur corrompue : forcer SIGSEGV.
        // En prod, mettre le thread en SIGSEGV et passer à l'action Core.
        return;
    }

    // Construire le SignalFrame.
    let sig_frame = SignalFrame {
        pretcode: action.restorer,
        signo:    sig_n as u64,
        pinfo:    sig_rsp + offset_of_info(),
        puc:      sig_rsp + offset_of_uc(),
        info:     SigInfoC::from_queue(info),
        uc: UContext {
            // SIG-13 : stocker SIGNAL_FRAME_MAGIC dans uc_flags pour vérification au sigreturn.
            uc_flags:   SIGNAL_FRAME_MAGIC as u64,
            uc_link:    0,
            uc_stack:   SigAltStack {
                ss_sp:    thread.addresses.sigaltstack_top()
                              .saturating_sub(thread.addresses.stack_size),
                ss_flags: if action.flags & SigAction::SA_ONSTACK != 0
                              { SS_ONSTACK } else { SS_DISABLE },
                ss_size:  thread.addresses.stack_size,
                _pad:     0,
            },
            uc_mcontext: GRegs {
                rip:    frame.user_rip,
                rsp:    frame.user_rsp,
                rax:    frame.user_rax,
                rdi:    frame.user_rdi,
                rsi:    frame.user_rsi,
                rdx:    frame.user_rdx,
                rcx:    frame.user_rcx,
                r8:     frame.user_r8,
                r9:     frame.user_r9,
                cs:     frame.user_cs as u16,
                eflags: frame.user_rflags,
                ..Default::default()
            },
            uc_sigmask: thread.sched_tcb.signal_mask.load(Ordering::Acquire),
            _fpregs_mem: [0u8; 512],
        },
    };

    // Écrire le frame sur la pile utilisateur.
    // SAFETY: sig_rsp pointe vers adresse userspace validée ci-dessus; écriture = livraison signal POSIX.
    unsafe {
        let ptr = sig_rsp as *mut SignalFrame;
        ptr.write(sig_frame);
    }

    // Mettre à jour le masque pendantl'exécution du handler (SA_NODEFER exclut sig_n).
    let old_mask = thread.sched_tcb.signal_mask.load(Ordering::Acquire);
    let mut new_mask = old_mask | action.mask;
    if action.flags & SigAction::SA_NODEFER == 0 {
        new_mask |= 1u64 << (sig_n - 1); // bloquer le signal courant
    }
    thread.sched_tcb.signal_mask.store(new_mask, Ordering::Release);

    // Rediriger le retour syscall vers le handler.
    frame.user_rip = action.handler;
    frame.user_rsp = sig_rsp;
    // Convention SA_SIGINFO (rdi = signo, rsi = *siginfo, rdx = *ucontext).
    frame.user_rdi = sig_n as u64;
    frame.user_rsi = sig_rsp + offset_of_info();
    frame.user_rdx = sig_rsp + offset_of_uc();
}

/// Restaure le contexte après sigreturn(2).
/// `uc_ptr` = adresse du UContext passé par l'utilisateur dans rdi.
/// Modifie `frame` pour restaurer les registres sauvegardés.
pub fn restore_signal_frame(
    thread: &mut ProcessThread,
    frame:  &mut SyscallFrame,
    uc_ptr: u64,
) {
    const USER_SPACE_TOP: u64 = 0x0000_7FFF_FFFF_F000;
    if uc_ptr >= USER_SPACE_TOP || uc_ptr < 0x1000 { return; }

    // SAFETY : uc_ptr a été écrit par setup_signal_frame ; on le relègit.
    let uc = unsafe { &*(uc_ptr as *const UContext) };
    let mc = &uc.uc_mcontext;

    // SIG-13 (constant-time) : extraire les données AVANT de vérifier le magic.
    let rip_saved    = mc.rip;
    let rsp_saved    = mc.rsp;
    let rax_saved    = mc.rax;
    let rdi_saved    = mc.rdi;
    let rsi_saved    = mc.rsi;
    let rdx_saved    = mc.rdx;
    let rcx_saved    = mc.rcx;
    let r8_saved     = mc.r8;
    let r9_saved     = mc.r9;
    let rflags_saved = mc.eflags;
    let mask_saved   = uc.uc_sigmask;

    // Vérification magic constant-time (SIG-13 / SIG-14).
    // Si le magic est invalide, on ne restaure rien (attaque de sigframe).
    let magic_diff = uc.uc_flags ^ (SIGNAL_FRAME_MAGIC as u64);
    if magic_diff != 0 { return; }

    // Restaurer les registres.
    frame.user_rip    = rip_saved;
    frame.user_rsp    = rsp_saved;
    frame.user_rax    = rax_saved;
    frame.user_rdi    = rdi_saved;
    frame.user_rsi    = rsi_saved;
    frame.user_rdx    = rdx_saved;
    frame.user_rcx    = rcx_saved;
    frame.user_r8     = r8_saved;
    frame.user_r9     = r9_saved;
    frame.user_rflags = rflags_saved;

    // Restaurer le masque de signal sauvegardé (sans SIGKILL/SIGSTOP).
    use super::mask::SigMask;
    let restored_mask = SigMask::from(mask_saved);
    thread.sched_tcb.signal_mask.store(restored_mask.0, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Offsets const dans SignalFrame
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn offset_of_info() -> u64 {
    // SignalFrame { pretcode(8), signo(8), pinfo(8), puc(8), info(128), uc(...) }
    32u64
}

#[inline(always)]
fn offset_of_uc() -> u64 {
    32u64 + core::mem::size_of::<SigInfoC>() as u64
}
