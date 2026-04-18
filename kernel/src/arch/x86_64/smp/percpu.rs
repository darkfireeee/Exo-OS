//! # arch/x86_64/smp/percpu.rs — Données per-CPU (GS segment)
//!
//! Chaque CPU possède une zone de données privée accessible via le registre GS.
//! Le layout GS est FIXE et partagé entre l'ASM (syscall_entry_asm, exception stubs)
//! et le code Rust.
//!
//! ## Layout GS (offsets fixes — NE PAS changer sans mettre à jour les stubs ASM)
//! ```
//! gs:[0x00]  = u64 : kernel_rsp    — pile syscall du thread courant (RSP kernel)
//! gs:[0x08]  = u64 : user_rsp      — RSP userspace sauvegardé lors d'un syscall
//! gs:[0x10]  = u64 : cpu_id        — identifiant logique du CPU (0-based)
//! gs:[0x18]  = u64 : lapic_id      — APIC ID du CPU courant
//! gs:[0x20]  = u64 : current_tcb   — pointeur TCB du thread courant
//! gs:[0x28]  = u64 : idle_rsp      — RSP de la pile idle de ce CPU
//! gs:[0x30]  = u64 : preempt_count — compteur de désactivation préemption
//! gs:[0x38]  = u64 : irq_depth     — profondeur d'imbrication IRQ
//! gs:[0x40]  = *   — réservé (futur)
//! ```


use core::sync::atomic::{AtomicU32, Ordering};
use crate::arch::x86_64::cpu::msr;

pub const MAX_CPUS: usize = 256;

// ── Compteur de CPUs en ligne ─────────────────────────────────────────────────

static ONLINE_CPU_COUNT: AtomicU32 = AtomicU32::new(0);

/// Retourne le nombre de CPUs actuellement initialisés (BSP + APs signalés)
#[inline(always)]
pub fn cpu_count() -> u32 {
    ONLINE_CPU_COUNT.load(Ordering::Acquire)
}

// ── Structure PerCpuData ──────────────────────────────────────────────────────

/// Données per-CPU — stockées dans la section .percpu ou dans un tableau statique
///
/// Alignée sur 64 octets pour éviter le false-sharing entre CPUs.
#[repr(C, align(64))]
pub struct PerCpuData {
    // Champs accédés depuis l'ASM — offsets FIXES (voir module doc)
    pub kernel_rsp:    u64,   // 0x00
    pub user_rsp:      u64,   // 0x08
    pub cpu_id:        u64,   // 0x10
    pub lapic_id:      u64,   // 0x18
    pub current_tcb:   u64,   // 0x20 (pointeur opaque, cast vers TCB extern)
    pub idle_rsp:      u64,   // 0x28
    pub preempt_count: u64,   // 0x30
    pub irq_depth:     u64,   // 0x38

    // Données Rust (offsets non contraints par l'ASM)
    pub online:        bool,
    pub bsp:           bool,
    pub nmi_in_progress: bool,

    /// Compteur de context switches sur ce CPU
    pub ctx_switch_count: u64,
    /// Compteurs IRQ par vecteur (256 entrées, partagé avec idt::irq_counter)
    pub irq_counts: [u64; 256],
    /// Horodatage TSC du dernier context switch
    pub last_switch_tsc: u64,
    /// Horodatage TSC du dernier tick timer
    pub last_tick_tsc:   u64,

    _pad: [u8; 0], // garantit l'alignement 64
}

impl PerCpuData {
    #[allow(dead_code)]
    const fn zeroed() -> Self {
        Self {
            kernel_rsp: 0, user_rsp: 0, cpu_id: 0, lapic_id: 0,
            current_tcb: 0, idle_rsp: 0, preempt_count: 0, irq_depth: 0,
            online: false, bsp: false, nmi_in_progress: false,
            ctx_switch_count: 0, irq_counts: [0u64; 256],
            last_switch_tsc: 0, last_tick_tsc: 0,
            _pad: [],
        }
    }
}

// ── Tableau global des données per-CPU ───────────────────────────────────────

/// Tableau statique — un PerCpuData par CPU possible
///
/// Initialisé à zéro au boot, puis configuré par `init_percpu_for_bsp/ap`.
/// Placé dans `.bss` — pas d'initialisation dynamique nécessaire.
#[repr(align(64))]
struct PerCpuTable([PerCpuData; MAX_CPUS]);

// SAFETY: PerCpuData ne contient pas de pointeurs interthread non-Sync
unsafe impl Sync for PerCpuTable {}

static PER_CPU_TABLE: PerCpuTable = PerCpuTable(
    // SAFETY: [0u8; size_of::<[PerCpuData; MAX_CPUS]>] valide pour #[repr(C,align(64))] zeros-init.
    unsafe { core::mem::transmute([0u8; core::mem::size_of::<[PerCpuData; MAX_CPUS]>()]) }
);

// ── Accès par CPU ID ──────────────────────────────────────────────────────────

/// Retourne une référence immuable aux données du CPU `cpu_id`
///
/// # Panics
/// Si `cpu_id >= MAX_CPUS`.
#[inline]
pub fn per_cpu(cpu_id: usize) -> &'static PerCpuData {
    &PER_CPU_TABLE.0[cpu_id]
}

/// Retourne une référence mutable aux données du CPU `cpu_id`
///
/// # Safety
/// L'appelant doit garantir qu'aucun autre contexte n'accède simultanément
/// aux données de ce CPU (invariant respecté car chaque CPU accède à ses propres données).
#[inline]
pub unsafe fn per_cpu_mut(cpu_id: usize) -> &'static mut PerCpuData {
    // SAFETY: PER_CPU_TABLE est un tableau statique mutable via UnsafeCell pattern ;
    // chaque CPU accède exclusivement à sa propre entrée.
    let ptr = PER_CPU_TABLE.0.as_ptr().add(cpu_id) as *mut PerCpuData;
    &mut *ptr
}

/// Retourne l'ID du CPU courant en lisant GS:[0x10]
#[inline]
pub fn current_cpu_id() -> u32 {
    let id: u64;
    // SAFETY: GS:[0x10] est initialisé lors de l'init percpu de ce CPU
    unsafe { core::arch::asm!("mov {}, gs:[0x10]", out(reg) id, options(nostack, nomem)); }
    id as u32
}

// ─────────────────────────────────────────────────────────────────────────────
// C ABI EXPORT — scheduler/core/preempt.rs interface (RÈGLE SCHED-01 DOC3)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne l'ID logique du CPU courant (C ABI export).
///
/// Appelé depuis `scheduler::core::preempt` pour accéder au compteur per-CPU.
/// Lit GS:[0x10] sans verrou (atomique par nature per-CPU).
///
/// # Safety
/// GS doit être initialisé sur ce CPU (garanti après `init_percpu_for_bsp/ap`).
#[no_mangle]
pub unsafe extern "C" fn arch_current_cpu() -> u32 {
    current_cpu_id()
}

/// Retourne une référence aux données per-CPU du CPU courant
///
/// # Safety
/// Doit être appelé depuis le contexte de ce CPU uniquement.
#[inline]
pub fn current_per_cpu() -> &'static PerCpuData {
    per_cpu(current_cpu_id() as usize)
}

/// Écrit le pointeur TCB courant dans `gs:[0x20]`.
///
/// Appelé par le scheduler juste après un context switch, afin que le
/// chemin syscall/exception retrouve toujours le bon thread courant.
#[inline(always)]
pub fn set_current_tcb(tcb: *mut crate::scheduler::core::task::ThreadControlBlock) {
    let tcb_ptr = tcb as u64;
    // SAFETY: GS pointe sur une zone per-CPU valide initialisée au boot.
    // L'offset 0x20 correspond au champ `current_tcb` du layout PerCpuData.
    unsafe {
        core::arch::asm!(
            "mov gs:[0x20], {}",
            in(reg) tcb_ptr,
            options(nostack, preserves_flags),
        );
    }
}

/// Lit la valeur courante du slot `gs:[0x20]` (`current_tcb`).
///
/// Utile pour instrumentation/debug et tests P2-7.
///
/// # Safety
/// GS doit déjà pointer sur une zone per-CPU valide.
#[inline(always)]
pub unsafe fn read_current_tcb() -> u64 {
    let tcb: u64;
    core::arch::asm!(
        "mov {}, gs:[0x20]",
        out(reg) tcb,
        options(nostack, preserves_flags),
    );
    tcb
}

/// Met à jour le slot `gs:[0x00]` avec la pile kernel du thread entrant.
///
/// INVARIANT P1-5 : appelé dans `context_switch()` après chaque sélection
/// du thread suivant, avant reprise d'exécution userspace/syscall.
///
/// # Safety
/// `kstack_top` doit être une adresse de pile kernel valide pour le thread
/// courant (alignement ABI, mapping kernel présent).
#[inline(always)]
pub unsafe fn set_kernel_rsp(kstack_top: u64) {
    core::arch::asm!(
        "mov gs:[0x00], {}",
        in(reg) kstack_top,
        options(nostack, preserves_flags),
    );
}

/// Lit la valeur courante du slot `gs:[0x00]`.
///
/// Utile pour instrumentation/debug et préparation des tests P2-7.
///
/// # Safety
/// GS doit déjà pointer sur une zone per-CPU valide.
#[inline(always)]
pub unsafe fn read_kernel_rsp() -> u64 {
    let rsp: u64;
    core::arch::asm!("mov {}, gs:[0x00]", out(reg) rsp, options(nostack, preserves_flags));
    rsp
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise les données per-CPU pour le BSP (CPU 0)
///
/// Appelé depuis `boot::early_init` avant tout autre usage de GS.
pub fn init_percpu_for_bsp(kernel_stack_top: u64, lapic_id: u32) {
    // SAFETY: accès exclusif au CPU 0 lors du boot (APs pas encore démarrés)
    let data = unsafe { per_cpu_mut(0) };
    data.cpu_id    = 0;
    data.lapic_id  = lapic_id as u64;
    // NOTE P1-5: initialisation du thread de boot uniquement.
    // Après le premier context switch, `set_kernel_rsp()` maintient
    // l'invariant gs:[0x00] == pile kernel du thread courant.
    data.kernel_rsp = kernel_stack_top;
    data.online    = true;
    data.bsp       = true;

    // Pointer GS_BASE vers cette structure
    let addr = data as *const PerCpuData as u64;
    // SAFETY: MSR_GS_BASE write depuis Ring 0
    unsafe { msr::write_msr(msr::MSR_GS_BASE, addr); }
    // MSR_KERNEL_GS_BASE = même adresse (SWAPGS échange les deux)
    // SAFETY: MSR_KERNEL_GS_BASE write
    unsafe { msr::write_msr(msr::MSR_KERNEL_GS_BASE, addr); }

    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}

/// Initialise les données per-CPU pour un AP
///
/// Appelé depuis le trampoline AP, avant l'activation du scheduler.
pub fn init_percpu_for_ap(cpu_id: u32, kernel_stack_top: u64, lapic_id: u32) {
    if cpu_id as usize >= MAX_CPUS { return; }
    // SAFETY: cpu_id unique, AP initialise ses propres données
    let data = unsafe { per_cpu_mut(cpu_id as usize) };
    data.cpu_id    = cpu_id as u64;
    data.lapic_id  = lapic_id as u64;
    // NOTE P1-5: valeur initiale pour le thread AP d'amorçage.
    // Le scheduler rafraîchit ensuite ce slot via `set_kernel_rsp()`.
    data.kernel_rsp = kernel_stack_top;
    data.online    = true;
    data.bsp       = false;

    let addr = data as *const PerCpuData as u64;
    // SAFETY: MSR writes depuis Ring 0 sur l'AP courant
    unsafe {
        msr::write_msr(msr::MSR_GS_BASE, addr);
        msr::write_msr(msr::MSR_KERNEL_GS_BASE, addr);
    }

    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}

// ── Préemption ────────────────────────────────────────────────────────────────

/// Désactive la préemption (incrémente preempt_count)
#[inline]
pub fn preempt_disable() {
    // SAFETY: accès GS:[0x30] depuis Ring 0, non-réentrant par construction
    unsafe { core::arch::asm!("addq $1, gs:[0x30]", options(nostack)); }
}

/// Active la préemption (décrémente preempt_count)
#[inline]
pub fn preempt_enable() {
    // SAFETY: accès GS:[0x30] depuis Ring 0
    unsafe { core::arch::asm!("subq $1, gs:[0x30]", options(nostack)); }
}

/// Retourne `true` si la préemption est désactivée sur le CPU courant
#[inline]
pub fn preempt_is_disabled() -> bool {
    let count: u64;
    // SAFETY: lecture GS:[0x30]
    unsafe { core::arch::asm!("mov {}, gs:[0x30]", out(reg) count, options(nostack, nomem)); }
    count != 0
}

// ── Préparation P2-7 : test gabarit kernel_rsp ──────────────────────────────

#[cfg(test)]
mod p2_7_gs_tests {
    use super::*;

    /// P2-7 / Test 1 — gs:[0x20] current_tcb : écriture puis relecture.
    ///
    /// Nécessite Ring 0 + GS per-CPU initialisé → ignoré sur host.
    /// Exécuter sous QEMU via le harness kernel.
    #[test]
    #[cfg_attr(not(target_os = "none"), ignore = "P2-7: Ring 0 + GS requis")]
    fn current_tcb_write_read_roundtrip() {
        let fake_tcb_a: u64 = 0xFFFF_8000_DEAD_0000;
        let fake_tcb_b: u64 = 0xFFFF_8000_BEEF_0000;

        unsafe {
            set_current_tcb(fake_tcb_a as *mut crate::scheduler::core::task::ThreadControlBlock);
            assert_eq!(
                read_current_tcb(),
                fake_tcb_a,
                "gs:[0x20] doit refléter la valeur écrite (TCB thread A)"
            );

            set_current_tcb(fake_tcb_b as *mut crate::scheduler::core::task::ThreadControlBlock);
            assert_eq!(
                read_current_tcb(),
                fake_tcb_b,
                "gs:[0x20] doit changer après switch vers thread B"
            );
        }
    }

    /// P2-7 / Test 2 — gs:[0x00] kernel_rsp : invariant mis à jour au switch.
    ///
    /// Nécessite Ring 0 + GS per-CPU initialisé → ignoré sur host.
    #[test]
    #[cfg_attr(not(target_os = "none"), ignore = "P2-7: Ring 0 + GS requis")]
    fn kernel_rsp_updated_on_switch() {
        let kstack_a: u64 = 0xFFFF_8000_0000_0000;
        let kstack_b: u64 = 0xFFFF_8000_0001_0000;
        let fake_tcb: u64 = 0xFFFF_8000_ABCD_0000;

        unsafe {
            set_kernel_rsp(kstack_a);
            assert_eq!(
                read_kernel_rsp(),
                kstack_a,
                "gs:[0x00] doit refléter kstack du thread A"
            );

            set_kernel_rsp(kstack_b);
            assert_eq!(
                read_kernel_rsp(),
                kstack_b,
                "gs:[0x00] doit changer après switch vers thread B"
            );

            // Vérifie que les deux slots sont indépendants
            set_current_tcb(fake_tcb as *mut crate::scheduler::core::task::ThreadControlBlock);
            assert_ne!(
                read_kernel_rsp(),
                read_current_tcb(),
                "gs:[0x00] et gs:[0x20] sont des slots distincts"
            );
        }
    }
}
