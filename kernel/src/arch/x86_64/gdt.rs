//! # arch/x86_64/gdt.rs — Global Descriptor Table
//!
//! Gère les GDT per-CPU avec les segments nécessaires pour x86_64 :
//! - Null descriptor (obligatoire)
//! - Kernel Code 64-bit
//! - Kernel Data 64-bit
//! - User Code 32-bit (compat mode)
//! - User Data 64-bit
//! - User Code 64-bit
//! - TSS (16 octets — deux descripteurs)
//!
//! ## Sélecteurs (ring 3 = bits 0–1 = 3, ring 0 = 0)
//! Compatible SYSCALL/SYSRET : la disposition des sélecteurs satisfait les
//! contraintes MSR STAR (SYSCALL CS/SS contigus, SYSRET CS/SS contigus).
//!
//! ## STAR Layout requis
//! - STAR[47:32] = SYSCALL CS/SS — KERNEL_CS:KERNEL_DS (contigus)
//! - STAR[63:48] = SYSRET CS/SS  — USER_CS32:USER_DS (contigus)
//!
//! Disposition des sélecteurs :
//! ```
//! 0x00 = NULL
//! 0x08 = KERNEL_CS  (Ring 0, 64-bit code)
//! 0x10 = KERNEL_DS  (Ring 0, 64-bit data)
//! 0x18 = USER_CS32  (Ring 3, 32-bit compat code)
//! 0x20 = USER_DS    (Ring 3, 64-bit data)      ← SS pour SYSRET
//! 0x28 = USER_CS64  (Ring 3, 64-bit code)      ← CS pour SYSRET
//! 0x30/0x38 = TSS   (2 descripteurs × 8 bytes)
//! ```


use core::sync::atomic::{AtomicBool, Ordering};

use super::cpu::topology::MAX_CPUS;
use super::tss;

// ── Sélecteurs GDT ────────────────────────────────────────────────────────────

pub const GDT_NULL:       u16 = 0x00;
pub const GDT_KERNEL_CS:  u16 = 0x08;
pub const GDT_KERNEL_DS:  u16 = 0x10;
pub const GDT_USER_CS32:  u16 = 0x18 | 3;   // Ring 3
pub const GDT_USER_DS:    u16 = 0x20 | 3;   // Ring 3
pub const GDT_USER_CS64:  u16 = 0x28 | 3;   // Ring 3
pub const GDT_TSS_SEL:    u16 = 0x30;        // TSS (Ring 0)

/// Valeur MSR STAR bits [47:32] = SYSCALL CS/SS (kernel)
/// SYSCALL : CS = STAR[47:32], SS = CS+8
pub const STAR_KERNEL_SEG: u32 = GDT_KERNEL_CS as u32;

/// Valeur MSR STAR bits [63:48] = SYSRET CS/SS (user)
/// SYSRET 64-bit : CS = STAR[63:48]+16, SS = STAR[63:48]+8
/// → USER_CS32 correspond ici, SYSRET 64 = USER_CS32+16 = USER_CS64
pub const STAR_USER_SEG:   u32 = GDT_USER_CS32 as u32 & !3;  // sans RPL

// ── Descripteur GDT 64 bits ───────────────────────────────────────────────────

/// Descripteur GDT 8 bytes
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct GdtDescriptor(u64);

impl GdtDescriptor {
    /// Constructeur générique de descripteur
    ///
    /// Pour les segments 64-bit, base et limite sont ignorés (toujours 0/0xFFFFF).
    const fn new(
        base:   u32,
        limit:  u32,
        access: u8,
        flags:  u8, // bits [7:4] du byte 6 : G, D/B, L, AVL
    ) -> Self {
        let limit_lo = limit & 0xFFFF;
        let base_lo  = (base & 0xFFFF) as u64;
        let base_mid = ((base >> 16) & 0xFF) as u64;
        let base_hi  = ((base >> 24) & 0xFF) as u64;
        let limit_hi = ((limit >> 16) & 0xF) as u64;
        let flags_nibble = (flags & 0xF) as u64;
        let access_byte  = access as u64;

        Self(
              limit_lo         as u64
            | (base_lo  << 16)
            | (base_mid << 32)
            | (access_byte << 40)
            | (limit_hi  << 48)
            | (flags_nibble << 52)
            | (base_hi  << 56)
        )
    }

    /// Descripteur segment nul
    const fn null() -> Self { Self(0) }

    /// Segment kernel code 64-bit (L=1, D/B=0)
    const fn kernel_code64() -> Self {
        // Access : P=1, DPL=0, S=1, Type=0xA (code exec+read)
        // Flags : G=1, L=1 (64-bit), D=0
        Self::new(0, 0x000F_FFFF, 0x9A, 0xA)
    }

    /// Segment kernel data 64-bit
    const fn kernel_data64() -> Self {
        // Access : P=1, DPL=0, S=1, Type=0x2 (data r/w)
        // Flags : G=1, B=1 (32-bit stack, ignoré en 64-bit)
        Self::new(0, 0x000F_FFFF, 0x92, 0xC)
    }

    /// Segment user code 32-bit (compat mode, DPL=3)
    const fn user_code32() -> Self {
        // Access : P=1, DPL=3, S=1, Type=0xA
        // Flags : G=1, D=1 (32-bit)
        Self::new(0, 0x000F_FFFF, 0xFA, 0xC)
    }

    /// Segment user data 64-bit (DPL=3)
    const fn user_data64() -> Self {
        // Access : P=1, DPL=3, S=1, Type=0x2
        Self::new(0, 0x000F_FFFF, 0xF2, 0xC)
    }

    /// Segment user code 64-bit (DPL=3, L=1)
    const fn user_code64() -> Self {
        // Access : P=1, DPL=3, S=1, Type=0xA
        // Flags : G=1, L=1
        Self::new(0, 0x000F_FFFF, 0xFA, 0xA)
    }

    /// Descripteur TSS (lower 8 bytes d'un System Segment 16-bytes)
    fn tss_lower(base: u64, limit: u32) -> Self {
        // Access : P=1, DPL=0, Type=0x9 (Available 64-bit TSS)
        let base_lo  = (base & 0xFFFF) as u64;
        let base_mid = ((base >> 16) & 0xFF) as u64;
        let base_hi  = ((base >> 24) & 0xFF) as u64;
        let limit_lo = (limit & 0xFFFF) as u64;
        let limit_hi = ((limit >> 16) & 0xF) as u64;
        let access   = 0x89u64; // P=1, DPL=0, Type=0x9

        Self(
              limit_lo
            | (base_lo  << 16)
            | (base_mid << 32)
            | (access   << 40)
            | (limit_hi  << 48)
            | (0xAu64    << 52) // G=1, flags
            | (base_hi   << 56)
        )
    }

    /// Descripteur TSS (upper 8 bytes — base[63:32] + réservé)
    fn tss_upper(base: u64) -> Self {
        // Contient uniquement base[63:32]
        Self((base >> 32) & 0xFFFF_FFFF)
    }
}

// ── Table GDT per-CPU ─────────────────────────────────────────────────────────

const GDT_ENTRIES: usize = 9; // NULL, KCODE, KDATA, UCODE32, UDATA, UCODE64, TSS×2

/// GDT d'un CPU (alignée 8 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, align(8))]
pub struct Gdt {
    entries: [GdtDescriptor; GDT_ENTRIES],
}

/// Registre GDTR
#[repr(C, packed)]
pub struct GdtRegister {
    limit: u16,
    base:  u64,
}

impl Gdt {
    const fn new() -> Self {
        Self {
            entries: [GdtDescriptor::null(); GDT_ENTRIES],
        }
    }

    fn install(&mut self, cpu_id: usize) {
        self.entries[0] = GdtDescriptor::null();
        self.entries[1] = GdtDescriptor::kernel_code64();
        self.entries[2] = GdtDescriptor::kernel_data64();
        self.entries[3] = GdtDescriptor::user_code32();
        self.entries[4] = GdtDescriptor::user_data64();
        self.entries[5] = GdtDescriptor::user_code64();

        // Installer le TSS pour ce CPU
        // SAFETY: cpu_id < MAX_CPUS vérifié par l'appelant (assert dans init_gdt_for_cpu).
        // CPU_TSS[cpu_id] est déjà initialisé par init_tss_for_cpu() appelé juste avant.
        let tss_addr = unsafe { tss::tss_ptr(cpu_id) as u64 };
        let tss_limit = (core::mem::size_of::<tss::TaskStateSegment>() - 1) as u32;
        self.entries[6] = GdtDescriptor::tss_lower(tss_addr, tss_limit);
        self.entries[7] = GdtDescriptor::tss_upper(tss_addr);
        self.entries[8] = GdtDescriptor::null(); // padding pour alignement
    }
}

// ── GDTs statiques per-CPU ────────────────────────────────────────────────────

static mut CPU_GDTS: [Gdt; MAX_CPUS] = [Gdt::new(); MAX_CPUS];
static GDT_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── API publique ──────────────────────────────────────────────────────────────

/// Initialise et charge la GDT pour le CPU `cpu_id`
///
/// Appelle `init_tss_for_cpu(cpu_id, kernel_stack_top)` en interne,
/// puis installe la GDT et charge le TSS dans TR.
///
/// # SAFETY
/// `cpu_id` doit correspondre au CPU courant.
/// `kernel_stack_top` doit pointer vers le sommet d'une pile kernel valide.
pub unsafe fn init_gdt_for_cpu(cpu_id: usize, kernel_stack_top: u64) {
    assert!(cpu_id < MAX_CPUS, "GDT: cpu_id hors bornes");

    // 1. Initialiser le TSS en premier (GDT en a besoin pour le descripteur)
    // SAFETY: cpu_id unique, kernel_stack_top pointe vers une pile valide
    tss::init_tss_for_cpu(cpu_id, kernel_stack_top);

    // SAFETY: cpu_id unique par CPU — pas de race entre CPUs différents
    let gdt = unsafe { &mut CPU_GDTS[cpu_id] };
    gdt.install(cpu_id);

    let gdtr = GdtRegister {
        limit: (core::mem::size_of::<Gdt>() - 1) as u16,
        base:  gdt.entries.as_ptr() as u64,
    };

    // 1. Charger GDTR
    // SAFETY: gdtr pointe vers une GDT valide — sûr de charger
    unsafe {
        core::arch::asm!(
            "lgdt [{gdtr}]",
            gdtr = in(reg) &gdtr as *const GdtRegister,
            options(nostack, nomem)
        );
    }

    // 2. Recharger les sélecteurs de segments (nécessaire après lgdt)
    // CS est rechargé via un far-return (RETFQ)
    // SAFETY: recharge des sélecteurs valides établis dans la GDT ci-dessus
    unsafe {
        core::arch::asm!(
            // Construire le far-return : push CS puis push RIP cible
            "push {kcs}",
            "lea  {tmp}, [rip + 1f]",
            "push {tmp}",
            "retfq",
            "1:",
            kcs = in(reg) GDT_KERNEL_CS as u64,
            tmp = out(reg) _,
            options(nostack)
        );

        // Recharger DS, ES, SS avec le sélecteur kernel data
        core::arch::asm!(
            "mov ax, {kds}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "xor ax, ax",    // FS = GS = 0 (seront repositionnés via MSR)
            "mov fs, ax",
            "mov gs, ax",
            kds = const GDT_KERNEL_DS,
            out("ax") _,
            options(nostack, nomem)
        );
    }

    // 3. Charger le TSS dans TR
    // SAFETY: GDT_TSS_SEL pointe vers un descripteur TSS valide
    unsafe { tss::load_tss(GDT_TSS_SEL); }

    GDT_INITIALIZED.store(true, Ordering::Release);
}

/// Retourne `true` si la GDT a été initialisée
#[inline(always)]
pub fn gdt_ready() -> bool {
    GDT_INITIALIZED.load(Ordering::Relaxed)
}

/// Valide que les sélecteurs sont conformes aux exigences de SYSCALL/SYSRET
///
/// Vérifie la disposition STAR requise.
pub fn validate_star_layout() -> bool {
    // SYSCALL : CS = STAR[47:32], +8 = SS
    // SYSRET 64 : CS = STAR[63:48]+16, SS = +8
    // Avec notre layout : KERNEL_CS(0x08), KERNEL_DS(0x10) → différence = 8 ✓
    // USER_CS32(0x18) → USER_DS(0x20) → USER_CS64(0x28) → diff = 8 ✓
    (GDT_KERNEL_DS - GDT_KERNEL_CS) == 8
        && (GDT_USER_DS  - GDT_USER_CS32) == 8
        && (GDT_USER_CS64 - GDT_USER_CS32) == 16
}
