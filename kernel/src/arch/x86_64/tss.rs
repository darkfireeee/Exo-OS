//! # arch/x86_64/tss.rs — Task State Segment (TSS) x86_64
//!
//! Gère les TSS per-CPU avec les piles IST pour la gestion des
//! exceptions critiques (Double Fault, NMI, Machine Check, etc.).
//!
//! ## Structure mémoire
//! - TSS 104 bytes (AMD64 ABI)
//! - 7 piles IST (Interrupt Stack Table) — 8 KiB chacune
//! - RSP0 : pile kernel (entrée Ring 3 → Ring 0)
//!
//! ## IST Assignments (Exo-OS)
//! - IST1 : Double Fault (#DF) — pile dédiée obligatoire
//! - IST2 : NMI — pile dédiée (NMI n'est pas ré-entrant)
//! - IST3 : Machine Check (#MC) — pile dédiée
//! - IST4-7 : disponibles

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, Ordering};

use super::cpu::topology::MAX_CPUS;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Taille d'une pile IST (8 KiB = 2 pages)
pub const IST_STACK_SIZE: usize = 8192;

/// Taille de la pile kernel RSP0 (16 KiB)
pub const KERNEL_STACK_SIZE: usize = 16384;

/// Nombre de piles IST dans le TSS
pub const IST_COUNT: usize = 7;

/// Index IST pour Double Fault
pub const IST_DOUBLE_FAULT: usize = 0; // IST1 (1-indexed dans IDT, 0-indexed ici)

/// Index IST pour NMI
pub const IST_NMI: usize = 1;

/// Index IST pour Machine Check
pub const IST_MACHINE_CHECK: usize = 2;

/// Index IST pour Debug (#DB)
pub const IST_DEBUG: usize = 3;

// ── Structure TSS ─────────────────────────────────────────────────────────────

/// TSS x86_64 (104 bytes, aligné 16 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct TaskStateSegment {
    _reserved0:  u32,
    /// Piles par niveau de privilège (RSP0 = entrée Ring 3 → Ring 0)
    pub rsp:     [u64; 3],   // RSP0, RSP1, RSP2
    _reserved1:  u64,
    /// Interrupt Stack Table (7 entrées)
    pub ist:     [u64; 7],   // IST1-IST7
    _reserved2:  u64,
    _reserved3:  u16,
    /// Offset de l'I/O Permission Bitmap (mettre à taille du TSS = pas de IOPB)
    pub iopb_offset: u16,
}

impl TaskStateSegment {
    pub const fn zero() -> Self {
        Self {
            _reserved0:  0,
            rsp:         [0u64; 3],
            _reserved1:  0,
            ist:         [0u64; 7],
            _reserved2:  0,
            _reserved3:  0,
            iopb_offset: core::mem::size_of::<TaskStateSegment>() as u16,
        }
    }
}

// ── Piles IST per-CPU ─────────────────────────────────────────────────────────

/// Piles IST pour un CPU (allouées statiquement)
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct PerCpuStacks {
    /// Pile Double Fault
    df_stack:  [u8; IST_STACK_SIZE],
    /// Pile NMI
    nmi_stack: [u8; IST_STACK_SIZE],
    /// Pile Machine Check
    mc_stack:  [u8; IST_STACK_SIZE],
    /// Pile Debug
    db_stack:  [u8; IST_STACK_SIZE],
    /// Pile IST5 (réservée)
    ist5_stack: [u8; IST_STACK_SIZE],
    /// Pile IST6 (réservée)
    ist6_stack: [u8; IST_STACK_SIZE],
    /// Pile IST7 (réservée)
    ist7_stack: [u8; IST_STACK_SIZE],
}

impl PerCpuStacks {
    const fn zero() -> Self {
        Self {
            df_stack:   [0u8; IST_STACK_SIZE],
            nmi_stack:  [0u8; IST_STACK_SIZE],
            mc_stack:   [0u8; IST_STACK_SIZE],
            db_stack:   [0u8; IST_STACK_SIZE],
            ist5_stack: [0u8; IST_STACK_SIZE],
            ist6_stack: [0u8; IST_STACK_SIZE],
            ist7_stack: [0u8; IST_STACK_SIZE],
        }
    }
}

/// TSS statiques per-CPU (limite : MAX_CPUS)
static mut CPU_TSS:    [TaskStateSegment; MAX_CPUS] = [TaskStateSegment::zero(); MAX_CPUS];
static mut CPU_STACKS: [PerCpuStacks;    MAX_CPUS] = const {
    let arr = [PerCpuStacks::zero(); MAX_CPUS];
    arr
};

static TSS_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise le TSS pour le CPU courant
///
/// `cpu_id` : identifiant logique du CPU (0 pour BSP)
/// `kernel_rsp0` : adresse du haut de pile kernel (RSP0)
///
/// # SAFETY
/// Doit être appelé depuis le CPU `cpu_id` après init de la GDT.
/// Le segment TSS doit avoir été chargé dans la GDT avant `ltr`.
pub fn init_tss_for_cpu(cpu_id: usize, kernel_rsp0: u64) {
    assert!(cpu_id < MAX_CPUS, "TSS: cpu_id hors bornes");

    // SAFETY: cpu_id unique par CPU — pas de course entre CPUs différents
    let tss = unsafe { &mut CPU_TSS[cpu_id] };
    // SAFETY: même invariant — CPU_STACKS[cpu_id] est exclusif à ce CPU.
    let stacks = unsafe { &CPU_STACKS[cpu_id] };

    // RSP0 : pile kernel pour les exceptions depuis userspace
    tss.rsp[0] = kernel_rsp0;

    // IST1–7 : sommet des piles dédiées (la pile croît vers le bas)
    let df_top  = stacks.df_stack.as_ptr()   as u64 + IST_STACK_SIZE as u64;
    let nmi_top = stacks.nmi_stack.as_ptr()  as u64 + IST_STACK_SIZE as u64;
    let mc_top  = stacks.mc_stack.as_ptr()   as u64 + IST_STACK_SIZE as u64;
    let db_top  = stacks.db_stack.as_ptr()   as u64 + IST_STACK_SIZE as u64;
    let ist5_top = stacks.ist5_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
    let ist6_top = stacks.ist6_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
    let ist7_top = stacks.ist7_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;

    tss.ist[IST_DOUBLE_FAULT]   = df_top;
    tss.ist[IST_NMI]            = nmi_top;
    tss.ist[IST_MACHINE_CHECK]  = mc_top;
    tss.ist[IST_DEBUG]          = db_top;
    tss.ist[4]                  = ist5_top;
    tss.ist[5]                  = ist6_top;
    tss.ist[6]                  = ist7_top;

    // IOPB : pointé au-delà du TSS → accès I/O interdit depuis Ring 3
    tss.iopb_offset = core::mem::size_of::<TaskStateSegment>() as u16;

    TSS_INITIALIZED.store(true, Ordering::Release);
}

/// Retourne un pointeur vers le TSS du CPU `cpu_id`
///
/// # SAFETY
/// `cpu_id` doit être < MAX_CPUS et le TSS doit avoir été initialisé.
pub unsafe fn tss_ptr(cpu_id: usize) -> *const TaskStateSegment {
    // SAFETY: délégué à l'appelant
    unsafe { &CPU_TSS[cpu_id] as *const _ }
}

/// Retourne un pointeur mutable vers le TSS du CPU `cpu_id`
///
/// # SAFETY
/// Idem `tss_ptr`. De plus, l'appelant garantit l'absence de race.
pub unsafe fn tss_ptr_mut(cpu_id: usize) -> *mut TaskStateSegment {
    // SAFETY: délégué à l'appelant
    unsafe { &mut CPU_TSS[cpu_id] as *mut _ }
}

/// Met à jour RSP0 dans le TSS courant (après context switch)
///
/// Doit être appelé à chaque entrée dans le kernel depuis un nouveau thread.
///
/// # SAFETY
/// `cpu_id` doit être le CPU courant. `rsp0` pointe vers le haut de pile
/// kernel du thread entrant.
#[inline(always)]
pub unsafe fn update_rsp0(cpu_id: usize, rsp0: u64) {
    // SAFETY: délégué à l'appelant — mise à jour atomique du champ rsp[0]
    unsafe { CPU_TSS[cpu_id].rsp[0] = rsp0; }
}

/// Charge le TSS dans le registre TR via LTR
///
/// # SAFETY
/// Le sélecteur TSS doit être valide dans la GDT courante.
/// Doit être appelé après `init_tss_for_cpu()`.
#[inline]
pub unsafe fn load_tss(tss_selector: u16) {
    // SAFETY: sélecteur valide garanti par l'appelant
    unsafe {
        core::arch::asm!(
            "ltr {sel:x}",
            sel = in(reg) tss_selector,
            options(nostack, nomem)
        );
    }
}

/// Retourne `true` si le TSS a été initialisé (au moins pour cpu_id 0)
#[inline(always)]
pub fn tss_ready() -> bool {
    TSS_INITIALIZED.load(Ordering::Relaxed)
}

// ── IST validation ────────────────────────────────────────────────────────────

/// Vérifie que toutes les piles IST sont correctement alignées
///
/// Retourne `Err(&str)` si une pile est mal alignée.
pub fn validate_ist_alignment(cpu_id: usize) -> Result<(), &'static str> {
    if cpu_id >= MAX_CPUS { return Err("cpu_id hors bornes"); }
    // SAFETY: cpu_id < MAX_CPUS vérifié ci-dessus ; CPU_TSS est un tableau statique
    // dont l'index cpu_id n'est accédé qu'en lecture ici — pas de race possible.
    // SAFETY: Lecture non-concurrente d'un TSS initialisé par ce CPU.
    // Utilise read_unaligned car TaskStateSegment est repr(packed).
    let tss_ptr = unsafe { &CPU_TSS[cpu_id] as *const TaskStateSegment };
    let ist_copy: [u64; 7] = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!((*tss_ptr).ist))
    };

    for (i, &ist_addr) in ist_copy.iter().enumerate() {
        if ist_addr == 0 { continue; }
        if ist_addr % 16 != 0 {
            return Err("IST stack non aligné sur 16 bytes");
        }
        let _ = i;
    }
    Ok(())
}
