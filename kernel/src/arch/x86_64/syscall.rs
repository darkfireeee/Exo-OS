//! # arch/x86_64/syscall.rs — SYSCALL/SYSRET entry point
//!
//! Gère l'entrée et la sortie des appels système en 64-bit via SYSCALL/SYSRET.
//!
//! ## Règle DOC1 (RÈGLE SIGNAL-01 / SIGNAL-02)
//! arch/ ORCHESTRE la livraison des signaux au retour vers userspace.
//! arch/ appelle `process::signal::delivery::handle_pending_signals()`.
//!
//! ## ABI Syscall Exo-OS (Linux-compatible)
//! - rax = numéro syscall
//! - rdi, rsi, rdx, r10, r8, r9 = arguments
//! - rcx = sauvé par SYSCALL (RIP de retour userspace)
//! - r11 = sauvé par SYSCALL (RFLAGS userspace)
//!
//! ## SWAPGS
//! - Entrée : SWAPGS pour accéder aux données per-CPU kernel
//! - Sortie  : SWAPGS avant SYSRET pour restaurer GS userspace
//!
//! ## KPTI
//! Le switch CR3 se fait dans le code ASM de bas niveau (switch_asm.s).
//! syscall.rs gère la logique Rust après le passage en mode kernel.

use core::sync::atomic::{AtomicU64, Ordering};

use super::cpu::msr;
use super::gdt::{GDT_KERNEL_CS, GDT_USER_CS32};

// ── Numéros syscall ───────────────────────────────────────────────────────────

pub const SYSCALL_READ: u64 = 0;
pub const SYSCALL_WRITE: u64 = 1;
pub const SYSCALL_OPEN: u64 = 2;
pub const SYSCALL_CLOSE: u64 = 3;
pub const SYSCALL_STAT: u64 = 4;
pub const SYSCALL_FSTAT: u64 = 5;
pub const SYSCALL_LSTAT: u64 = 6;
pub const SYSCALL_POLL: u64 = 7;
pub const SYSCALL_LSEEK: u64 = 8;
pub const SYSCALL_MMAP: u64 = 9;
pub const SYSCALL_MPROTECT: u64 = 10;
pub const SYSCALL_MUNMAP: u64 = 11;
pub const SYSCALL_BRK: u64 = 12;
pub const SYSCALL_RT_SIGACTION: u64 = 13;
pub const SYSCALL_RT_SIGPROCMASK: u64 = 14;
pub const SYSCALL_RT_SIGRETURN: u64 = 15;
pub const SYSCALL_IOCTL: u64 = 16;
pub const SYSCALL_FORK: u64 = 57;
pub const SYSCALL_VFORK: u64 = 58;
pub const SYSCALL_EXECVE: u64 = 59;
pub const SYSCALL_EXIT: u64 = 60;
pub const SYSCALL_WAIT4: u64 = 61;
pub const SYSCALL_KILL: u64 = 62;
pub const SYSCALL_CLONE: u64 = 56;
pub const SYSCALL_FUTEX: u64 = 202;
pub const SYSCALL_SCHED_YIELD: u64 = 24;
pub const SYSCALL_NANOSLEEP: u64 = 35;
pub const SYSCALL_GETPID: u64 = 39;
pub const SYSCALL_SOCKET: u64 = 41;
pub const SYSCALL_CONNECT: u64 = 42;
pub const SYSCALL_ACCEPT: u64 = 43;
pub const SYSCALL_SENDTO: u64 = 44;
pub const SYSCALL_RECVFROM: u64 = 45;

/// Numéro syscall maximum supporté
pub const SYSCALL_MAX: u64 = 512;

// ── Frame syscall ─────────────────────────────────────────────────────────────

/// Registres sauvegardés à l'entrée SYSCALL
///
/// ## Layout mémoire (DOIT correspondre à l'ordre push ASM)
/// Les champs sont dans l'ordre croissant d'adresse : rax est au plus bas
/// (dernier push = [rsp+0]), rcx est au plus haut (premier push = [rsp+120]).
///
/// Ordre des pushs ASM (premier à dernier) :
///   push rcx  → [rsp+120]  (RIP retour)
///   push r11  → [rsp+112]  (RFLAGS userspace)
///   push rbp  → [rsp+104]
///   push rbx  → [rsp+96]
///   push r12  → [rsp+88]
///   push r13  → [rsp+80]
///   push r14  → [rsp+72]
///   push r15  → [rsp+64]
///   push user_rsp → [rsp+56]
///   push rsi  → [rsp+48]   (arg2)
///   push rdi  → [rsp+40]   (arg1)
///   push rdx  → [rsp+32]   (arg3)
///   push r10  → [rsp+24]   (arg4)
///   push r8   → [rsp+16]   (arg5)
///   push r9   → [rsp+8]    (arg6)
///   push rax  → [rsp+0]    (syscall nr / retour)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SyscallFrame {
    pub rax: u64, // offset   0 — syscall number / return value (last push)
    pub r9: u64,  // offset   8 — arg6
    pub r8: u64,  // offset  16 — arg5
    pub r10: u64, // offset  24 — arg4
    pub rdx: u64, // offset  32 — arg3
    pub rdi: u64, // offset  40 — arg1
    pub rsi: u64, // offset  48 — arg2
    pub rsp: u64, // offset  56 — RSP userspace sauvé
    pub r15: u64, // offset  64
    pub r14: u64, // offset  72
    pub r13: u64, // offset  80
    pub r12: u64, // offset  88
    pub rbx: u64, // offset  96
    pub rbp: u64, // offset 104
    pub r11: u64, // offset 112 — RFLAGS userspace (sauvé par SYSCALL hw)
    pub rcx: u64, // offset 120 — RIP retour userspace (first push)
}

// ── Initialisation SYSCALL/SYSRET ─────────────────────────────────────────────

/// Configure les MSRs pour SYSCALL/SYSRET
///
/// Doit être appelé sur chaque CPU après init GDT.
pub fn init_syscall() {
    use msr::*;

    // 1. Activer SCE dans EFER (SYSCALL Enable)
    // SAFETY: EFER.SCE est sûr à activer une fois la GDT en place
    unsafe {
        set_msr_bits(MSR_IA32_EFER, EFER_SCE);
    }

    // 2. MSR STAR : sélecteurs segments
    // Bits [47:32] = SYSCALL CS (kernel CS) ; SS = CS+8 = KERNEL_DS
    // Bits [63:48] = SYSRET CS32 ; SYSRET 64 CS = +16, SS = +8
    //
    // Note : STAR.SYSRET_CS pointe sur USER_CS32 (ring 3).
    // SYSRET 64-bit utilisera USER_CS32+16 = USER_CS64 pour CS
    // et USER_CS32+8 = USER_DS pour SS.
    let star_kernel = (GDT_KERNEL_CS as u64) << 32;
    let star_user = ((GDT_USER_CS32 & !3) as u64) << 48;
    // SAFETY: STAR contient uniquement des sélecteurs GDT valides
    unsafe {
        write_msr(MSR_STAR, star_kernel | star_user);
    }

    // 3. MSR LSTAR : adresse 64-bit du handler SYSCALL
    // SAFETY: syscall_entry_asm est le point d'entrée syscall valide
    unsafe {
        write_msr(MSR_LSTAR, syscall_entry_asm as *const () as u64);
    }

    // 4. MSR CSTAR : handler mode compat (non utilisé — pointe vers une fonction vide)
    // SAFETY: syscall_cstar_asm est une fonction noop valide
    unsafe {
        write_msr(MSR_CSTAR, syscall_cstar_noop as *const () as u64);
    }

    // 5. MSR SFMASK : masque RFLAGS lors de SYSCALL
    // Masquer IF (bit 9), TF (bit 8), DF (bit 10), AC (bit 18)
    let sfmask = (1 << 9)  // IF
               | (1 << 8)  // TF
               | (1 << 10) // DF
               | (1 << 18); // AC
                            // SAFETY: SFMASK masque les flags dangereux uniquement
    unsafe {
        write_msr(MSR_SFMASK, sfmask);
    }
}

// ── Entrée SYSCALL en ASM ─────────────────────────────────────────────────────

// Point d'entrée SYSCALL 64-bit — défini en global_asm!
//
// Séquence :
// 1. SWAPGS (kernel GS)
// 2. Sauvegarder RSP userspace dans per-CPU area
// 3. Charger RSP0 kernel depuis per-CPU area
// 4. Sauvegarder tous les registres caller-saved + RCX/R11
// 5. Appeler `syscall_rust_handler()`
// 6. Restaurer les registres
// 7. SWAPGS (restaurer GS userspace)
// 8. SYSRETQ
core::arch::global_asm!(
    ".section .text",
    ".global syscall_entry_asm",
    ".type   syscall_entry_asm, @function",
    "syscall_entry_asm:",
    // ── 1. SWAPGS : active le GS kernel (kernel GS_BASE ← KERNEL_GS_BASE) ──────
    "swapgs",
    // ── 2. Sauvegarder RSP userspace dans gs:[0x08] (user_rsp slot) ─────────────
    // gs:[0x00] = kernel_rsp (pile kernel pré-allouée)
    // gs:[0x08] = user_rsp  (save slot pour le RSP userspace courant)
    "mov   qword ptr gs:[0x08], rsp",
    // ── 3. Charger la pile kernel depuis gs:[0x00] (kernel_rsp slot) ─────────────
    "mov   rsp, qword ptr gs:[0x00]",
    // ── 4. Construire la SyscallFrame sur la pile kernel ──────────────────────────
    // Ordre push : premier → haut adresse, dernier → bas adresse = [rsp]
    // DOIT correspondre exactement à l'ordre des champs de SyscallFrame
    "push  rcx",                 // [rsp+120] RIP retour userspace (SYSCALL saves in rcx)
    "push  r11",                 // [rsp+112] RFLAGS userspace   (SYSCALL saves in r11)
    "push  rbp",                 // [rsp+104]
    "push  rbx",                 // [rsp+ 96] (rbx sera écrasé temporairement — voir étape 6)
    "push  r12",                 // [rsp+ 88]
    "push  r13",                 // [rsp+ 80]
    "push  r14",                 // [rsp+ 72]
    "push  r15",                 // [rsp+ 64]
    "push  qword ptr gs:[0x08]", // [rsp+ 56] RSP userspace (lu depuis le save slot)
    "push  rsi",                 // [rsp+ 48] arg2
    "push  rdi",                 // [rsp+ 40] arg1
    "push  rdx",                 // [rsp+ 32] arg3
    "push  r10",                 // [rsp+ 24] arg4
    "push  r8",                  // [rsp+ 16] arg5
    "push  r9",                  // [rsp+  8] arg6
    "push  rax",                 // [rsp+  0] numéro syscall (frame.rax)
    // ── 5. Sauvegarder le pointeur de frame dans rbx (callee-saved) ──────────────
    // rbx est callee-saved : syscall_rust_handler le préservera.
    // La valeur sauvée sur frame (frame.rbx) est déjà sur la pile à [rsp+96].
    // On peut donc utiliser rbx librement comme pointeur de frame jusqu'au pop.
    "mov   rbx, rsp",
    // ── 6. Aligner la pile sur 16 bytes pour l'ABI AMD64 ─────────────────────────
    // Après 16 pushes de 8 bytes = 128 bytes (multiple de 16), rsp est déjà aligné.
    // Cette instruction est une précaution si kernel_rsp initial n'était pas aligné.
    "and   rsp, -16",
    // ── 7. Appeler le handler Rust avec SyscallFrame* comme premier argument ──────
    "mov   rdi, rbx",
    "call  syscall_rust_handler",
    // ── 8. Restaurer rsp au début de la frame (annule l'alignement éventuel) ──────
    // rbx est préservé par syscall_rust_handler (callee-saved ABI)
    "mov   rsp, rbx",
    // ── 9. Restaurer les registres depuis la frame (ordre inverse des pushs) ──────
    // frame.rax contient la valeur retour writée par le handler Rust
    "mov   rax, [rsp]", // return value = frame.rax
    "add   rsp, 8",     // skip rax slot
    "pop   r9",         // restore r9
    "pop   r8",         // restore r8
    "pop   r10",        // restore r10
    "pop   rdx",        // restore rdx
    "pop   rdi",        // restore rdi
    "pop   rsi",        // restore rsi
    "add   rsp, 8",     // skip user_rsp slot (restauré depuis GS ci-dessous)
    "pop   r15",
    "pop   r14",
    "pop   r13",
    "pop   r12",
    "pop   rbx", // restaure rbx original (user rbx depuis frame.rbx)
    "pop   rbp",
    "pop   r11", // RFLAGS userspace
    "pop   rcx", // RIP retour userspace
    // ── 10. Restaurer RSP userspace depuis gs:[0x08] ──────────────────────────────
    "mov   rsp, qword ptr gs:[0x08]",
    // ── 11. Restaurer GS userspace ───────────────────────────────────────────────
    "swapgs",
    // ── 12. Retour en mode 64-bit Ring 3 ─────────────────────────────────────────
    "sysretq",
    ".size syscall_entry_asm, . - syscall_entry_asm",
);

// Noop CSTAR pour SYSCALL compat mode (non supporté)
core::arch::global_asm!(
    ".section .text",
    ".global syscall_cstar_noop",
    ".type   syscall_cstar_noop, @function",
    "syscall_cstar_noop:",
    // Activer GS kernel pour accéder à la zone per-CPU.
    "swapgs",
    // Sauvegarder le RSP userspace dans gs:[0x08] (save slot standard).
    // RSP n'a PAS encore été changé (SYSCALL compat ne touche pas RSP).
    "mov qword ptr gs:[0x08], rsp",
    // Charger le RSP kernel depuis gs:[0x00] pour éviter tout travail sur la pile user.
    "mov rsp, qword ptr gs:[0x00]",
    // Retourner -ENOSYS (errno 38 = ENOSYS en Linux ABI).
    "mov eax, -38",
    // Restaurer RSP userspace depuis le save slot.
    "mov rsp, qword ptr gs:[0x08]",
    // Restaurer GS userspace.
    "swapgs",
    // Retour compat 32-bit : `sysret` sans suffixe émet la variante compat,
    // contrairement à `sysretq` qui force le retour 64-bit et provoquerait un #GP.
    "sysret",
    ".size syscall_cstar_noop, . - syscall_cstar_noop",
);

extern "C" {
    fn syscall_entry_asm();
    fn syscall_cstar_noop();
}

// ── Vérification adresse canonique x86_64 (RÈGLE ARCH-SYSRET / V-35) ─────────

/// Retourne `true` si l'adresse est canonique (bits 63..47 identiques).
/// Une adresse non-canonique dans RCX au moment de SYSRET cause un kernel fault
/// sur certains CPUs Intel/AMD — on l'intercepte ici pour l'envoyer en Ring 3.
#[inline(always)]
fn is_canonical(addr: u64) -> bool {
    let sign_bits = addr >> 47;
    sign_bits == 0 || sign_bits == 0x1FFFF
}

// ── Handler Rust SYSCALL ──────────────────────────────────────────────────────

/// Table de dispatch syscall
/// Indexée par numéro syscall, retourne le handler ou un défaut
#[allow(dead_code)]
type SyscallFn = fn(u64, u64, u64, u64, u64, u64) -> i64;

/// Handler appelé depuis l'ASM avec la SyscallFrame
///
/// # SAFETY
/// Appelé uniquement depuis `syscall_entry_asm` avec une `SyscallFrame` valide
/// sur la pile kernel.
#[no_mangle]
pub extern "C" fn syscall_rust_handler(frame: *mut SyscallFrame) {
    // SAFETY: frame est une SyscallFrame valide construite par l'ASM d'entrée
    let frame = unsafe { &mut *frame };

    SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);

    // ── RÈGLE SIGNAL-01 (DOC1) ────────────────────────────────────────────────
    // Le pipeline complet (fast-path → compat → table → signal delivery)
    // est géré par crate::syscall::dispatch::dispatch().
    // arch/ orchestre uniquement : le frame est passé par pointeur brut,
    // dispatch() lit les arguments et écrit frame.rax.
    // La livraison des signaux se fait dans post_dispatch() avant le retour.

    // SAFETY: frame provient de syscall_rust_handler qui reçoit un pointeur
    // valide depuis l'ASM. La durée de vie est bornée à cette stackframe.
    crate::syscall::dispatch::dispatch(frame);

    // ── ARCH-SYSRET (V-35) : vérification canonique de l'adresse de retour ──
    // Si frame.rcx (RIP retour userspace) n'est pas canonique, SYSRETQ causera
    // un fault en Ring 0 (errata Intel/AMD).
    // On force frame.rcx = 0 → processus faultera en Ring 3 à @0 (SIGSEGV),
    // jamais en Ring 0. Solution minimale sans dépendance process::signal ici.
    if !is_canonical(frame.rcx) {
        frame.rcx = 0; // adresse 0 = non-mappée → #PF userspace, pas kernel
    }
}

// ── Instrumentation syscall ───────────────────────────────────────────────────

static SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);
static SYSCALL_ERROR_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn syscall_count() -> u64 {
    SYSCALL_COUNT.load(Ordering::Relaxed)
}
pub fn syscall_error_count() -> u64 {
    SYSCALL_ERROR_COUNT.load(Ordering::Relaxed)
}
