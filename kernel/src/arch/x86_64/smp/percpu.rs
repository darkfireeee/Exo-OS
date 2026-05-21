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
//! gs:[0x30]  = u64 : preempt_shadow — miroir du compteur canonique scheduler
//! gs:[0x38]  = u64 : irq_depth     — profondeur d'imbrication IRQ
//! gs:[0x40]  = u64 : kpti_kernel_cr3 — CR3 kernel pour l'entrée syscall
//! gs:[0x48]  = u64 : kpti_user_cr3   — CR3 user pour la sortie syscall
//! gs:[0x50]  = u64 : syscall_scratch — scratch ASM pendant switch CR3
//! ```

use crate::arch::x86_64::cpu::msr;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

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
    pub kernel_rsp: u64,      // 0x00
    pub user_rsp: u64,        // 0x08
    pub cpu_id: u64,          // 0x10
    pub lapic_id: u64,        // 0x18
    pub current_tcb: u64,     // 0x20 (pointeur opaque, cast vers TCB extern)
    pub idle_rsp: u64,        // 0x28
    pub preempt_shadow: u64,  // 0x30 miroir du compteur canonique scheduler
    pub irq_depth: u64,       // 0x38
    pub kpti_kernel_cr3: u64, // 0x40
    pub kpti_user_cr3: u64,   // 0x48
    pub syscall_scratch: u64, // 0x50

    // Données Rust (offsets non contraints par l'ASM)
    pub online: bool,
    pub bsp: bool,
    pub nmi_in_progress: bool,

    /// Compteur de context switches sur ce CPU
    pub ctx_switch_count: u64,
    /// Compteurs IRQ par vecteur (256 entrées, partagé avec idt::irq_counter)
    pub irq_counts: [u64; 256],
    /// Horodatage TSC du dernier context switch
    pub last_switch_tsc: u64,
    /// Horodatage TSC du dernier tick timer
    pub last_tick_tsc: u64,

    _pad: [u8; 0], // garantit l'alignement 64
}

const _: () = assert!(
    core::mem::size_of::<PerCpuData>() <= 4096,
    "PerCpuData must fit in one page"
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, current_tcb) == 0x20,
    "PerCpuData.current_tcb must stay at GS:0x20"
);
const _: () = assert!(
    core::mem::offset_of!(PerCpuData, preempt_shadow) == 0x30,
    "PerCpuData.preempt_shadow must stay at GS:0x30"
);

impl PerCpuData {
    #[allow(dead_code)]
    const fn zeroed() -> Self {
        Self {
            kernel_rsp: 0,
            user_rsp: 0,
            cpu_id: 0,
            lapic_id: 0,
            current_tcb: 0,
            idle_rsp: 0,
            preempt_shadow: 0,
            irq_depth: 0,
            kpti_kernel_cr3: 0,
            kpti_user_cr3: 0,
            syscall_scratch: 0,
            online: false,
            bsp: false,
            nmi_in_progress: false,
            ctx_switch_count: 0,
            irq_counts: [0u64; 256],
            last_switch_tsc: 0,
            last_tick_tsc: 0,
            _pad: [],
        }
    }
}

// ── Tableau global des données per-CPU ───────────────────────────────────────

/// Tableau statique — un PerCpuData par CPU possible
///
/// Initialisé à zéro au boot, puis configuré par `init_percpu_for_bsp/ap`.
/// Place dans `.data.percpu` pour rester hors de `.rodata`: ces slots sont
/// mutables par construction et participent au chemin syscall/IRQ.
#[repr(align(64))]
struct PerCpuTable(UnsafeCell<[PerCpuData; MAX_CPUS]>);

// SAFETY: les acces mutables sont disciplines par CPU via `per_cpu_mut()`.
unsafe impl Sync for PerCpuTable {}

#[link_section = ".data.percpu"]
static PER_CPU_TABLE: PerCpuTable = PerCpuTable(
    // SAFETY: [0u8; size_of::<[PerCpuData; MAX_CPUS]>] valide pour #[repr(C,align(64))] zeros-init.
    UnsafeCell::new(unsafe {
        core::mem::transmute([0u8; core::mem::size_of::<[PerCpuData; MAX_CPUS]>()])
    }),
);

#[inline(always)]
fn per_cpu_ptr(cpu_id: usize) -> *mut PerCpuData {
    let low_ptr = unsafe { (*PER_CPU_TABLE.0.get()).as_mut_ptr().add(cpu_id) };

    #[cfg(target_os = "none")]
    {
        crate::memory::phys_to_virt(crate::memory::PhysAddr::new(low_ptr as u64)).as_u64()
            as *mut PerCpuData
    }

    #[cfg(not(target_os = "none"))]
    {
        low_ptr
    }
}

// ── Accès par CPU ID ──────────────────────────────────────────────────────────

/// Retourne une référence immuable aux données du CPU `cpu_id`
///
/// # Panics
/// Si `cpu_id >= MAX_CPUS`.
#[inline]
pub fn per_cpu(cpu_id: usize) -> &'static PerCpuData {
    // SAFETY: lecture partagee d'un slot per-CPU. Les mutations de ce slot sont
    // reservees au CPU proprietaire ou aux phases d'initialisation exclusives.
    unsafe { &*per_cpu_ptr(cpu_id) }
}

/// Retourne une référence mutable aux données du CPU `cpu_id`
///
/// # Safety
/// L'appelant doit garantir qu'aucun autre contexte n'accède simultanément
/// aux données de ce CPU (invariant respecté car chaque CPU accède à ses propres données).
#[inline]
pub unsafe fn per_cpu_mut(cpu_id: usize) -> &'static mut PerCpuData {
    // SAFETY: PER_CPU_TABLE est un tableau statique mutable via UnsafeCell ;
    // chaque CPU accede exclusivement a sa propre entree.
    &mut *per_cpu_ptr(cpu_id)
}

/// Retourne l'ID du CPU courant en lisant GS:[0x10]
#[inline]
pub fn current_cpu_id() -> u32 {
    let id: u64;
    // SAFETY: GS:[0x10] est initialisé lors de l'init percpu de ce CPU
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) id, options(nostack, nomem));
    }
    id as u32
}

#[inline(always)]
fn percpu_table_bounds() -> (u64, u64) {
    let base = per_cpu_ptr(0) as u64;
    let len = (core::mem::size_of::<PerCpuData>() * MAX_CPUS) as u64;
    (base, base.saturating_add(len))
}

#[inline(always)]
pub fn is_kernel_percpu_gs_base(gs_base: u64) -> bool {
    let (base, end) = percpu_table_bounds();
    gs_base >= base && gs_base < end
}

#[inline(always)]
pub fn try_current_cpu_id() -> Option<u32> {
    // SAFETY: lecture de MSR privilégiée en Ring 0. On vérifie la base GS avant
    // tout accès `gs:[...]` afin d'éviter une faute récursive en handler.
    let gs_base = unsafe { msr::read_msr(msr::MSR_GS_BASE) };
    if !is_kernel_percpu_gs_base(gs_base) {
        return None;
    }
    Some(current_cpu_id())
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

/// Publie le miroir du compteur de préemption canonique dans GS:[0x30].
///
/// Ce slot existe uniquement pour le code ASM bas niveau; la source de vérité
/// reste `scheduler::core::preempt::PREEMPT_COUNT`.
#[no_mangle]
pub extern "C" fn arch_set_preempt_count_shadow(depth: i32) {
    let value = depth.max(0) as u64;
    // SAFETY: GS pointe sur le slot per-CPU courant; offset 0x30 = preempt_shadow.
    unsafe {
        core::arch::asm!(
            "mov gs:[0x30], {}",
            in(reg) value,
            options(nostack, preserves_flags),
        );
    }
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

/// Lit `current_tcb` uniquement si GS pointe sur une zone per-CPU kernel valide.
///
/// Les handlers d'exception peuvent être appelés dans des fenêtres sensibles
/// (retour syscall/KPTI, faute noyau imbriquée). Dans ces chemins, lire
/// directement `gs:[0x20]` peut transformer la faute initiale en cascade sur
/// `CR2=0x20`. Cette API rend le contrat explicite.
#[inline(always)]
pub unsafe fn try_read_current_tcb() -> Option<u64> {
    let gs_base = unsafe { msr::read_msr(msr::MSR_GS_BASE) };
    if !is_kernel_percpu_gs_base(gs_base) {
        return None;
    }
    Some(unsafe { read_current_tcb() })
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

/// Publie les CR3 utilisés par le stub syscall KPTI.
#[inline(always)]
pub fn set_kpti_cr3_slots(kernel_cr3: u64, user_cr3: u64) {
    // SAFETY: GS pointe sur une zone per-CPU valide. Les offsets 0x40/0x48
    // sont lus directement par syscall_entry_asm avant/après le handler Rust.
    unsafe {
        core::arch::asm!(
            "mov gs:[0x40], {kernel}",
            "mov gs:[0x48], {user}",
            kernel = in(reg) kernel_cr3,
            user = in(reg) user_cr3,
            options(nostack, preserves_flags),
        );
    }
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise les données per-CPU pour le BSP (CPU 0)
///
/// Appelé depuis `boot::early_init` avant tout autre usage de GS.
pub fn init_percpu_for_bsp(kernel_stack_top: u64, lapic_id: u32) {
    // SAFETY: accès exclusif au CPU 0 lors du boot (APs pas encore démarrés)
    let data = unsafe { per_cpu_mut(0) };
    data.cpu_id = 0;
    data.lapic_id = lapic_id as u64;
    // NOTE P1-5: initialisation du thread de boot uniquement.
    // Après le premier context switch, `set_kernel_rsp()` maintient
    // l'invariant gs:[0x00] == pile kernel du thread courant.
    data.kernel_rsp = kernel_stack_top;
    data.online = true;
    data.bsp = true;

    // Pointer GS_BASE vers l'alias physmap de cette structure.
    let addr = data as *const PerCpuData as u64;
    // SAFETY: MSR_GS_BASE write depuis Ring 0
    unsafe {
        msr::write_msr(msr::MSR_GS_BASE, addr);
    }
    // MSR_KERNEL_GS_BASE contient le GS userspace "shadow".
    // Au boot il n'existe pas encore de TLS userspace valide, donc la valeur
    // initiale doit rester nulle et surtout pas pointer vers la zone per-CPU.
    // SAFETY: MSR_KERNEL_GS_BASE write
    unsafe {
        msr::write_msr(msr::MSR_KERNEL_GS_BASE, 0);
    }
    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}

/// Initialise les données per-CPU pour un AP
///
/// Appelé depuis le trampoline AP, avant l'activation du scheduler.
pub fn init_percpu_for_ap(cpu_id: u32, kernel_stack_top: u64, lapic_id: u32) {
    if cpu_id as usize >= MAX_CPUS {
        return;
    }
    // SAFETY: cpu_id unique, AP initialise ses propres données
    let data = unsafe { per_cpu_mut(cpu_id as usize) };
    data.cpu_id = cpu_id as u64;
    data.lapic_id = lapic_id as u64;
    // NOTE P1-5: valeur initiale pour le thread AP d'amorçage.
    // Le scheduler rafraîchit ensuite ce slot via `set_kernel_rsp()`.
    data.kernel_rsp = kernel_stack_top;
    data.online = true;
    data.bsp = false;

    let addr = data as *const PerCpuData as u64;
    // SAFETY: MSR writes depuis Ring 0 sur l'AP courant
    unsafe {
        msr::write_msr(msr::MSR_GS_BASE, addr);
        msr::write_msr(msr::MSR_KERNEL_GS_BASE, 0);
    }
    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}

// ── Préemption ────────────────────────────────────────────────────────────────

/// Désactive la préemption (incrémente preempt_count)
#[inline]
#[deprecated(note = "utiliser scheduler::core::preempt::PreemptGuard")]
pub fn preempt_disable() {
    crate::scheduler::core::preempt::preempt_disable_for_arch_compat();
}

/// Active la préemption (décrémente preempt_count)
#[inline]
#[deprecated(note = "utiliser le Drop de scheduler::core::preempt::PreemptGuard")]
pub fn preempt_enable() {
    crate::scheduler::core::preempt::preempt_enable_for_arch_compat();
}

/// Retourne `true` si la préemption est désactivée sur le CPU courant
#[inline]
#[deprecated(note = "utiliser scheduler::core::preempt::is_preempt_disabled")]
pub fn preempt_is_disabled() -> bool {
    preempt_is_disabled_canonical()
}

/// Lecture canonique de l'état de préemption.
#[inline]
pub fn preempt_is_disabled_canonical() -> bool {
    crate::scheduler::core::preempt::is_preempt_disabled()
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
