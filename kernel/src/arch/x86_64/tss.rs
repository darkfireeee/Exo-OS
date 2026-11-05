//! # arch/x86_64/tss.rs — Task State Segment (TSS) x86_64
//!
//! Manages per-CPU TSS with IST stacks for handling
//! critical exceptions (Double Fault, NMI, Machine Check, etc.).
//!
//! ## Memory Structure
//! - TSS 104 bytes (AMD64 ABI)
//! - 4 static IST stacks + 3 guarded early-pool stacks
//! - RSP0: kernel stack (Ring 3 → Ring 0 entry)
//!
//! ## IST Assignments (Exo-OS)
//! - IST1: ExoPhoenix IPIs (0xF0/0xF1/0xF2)
//! - IST2: #PF (hardening)
//! - IST3: NMI fallback (early-pool, guarded)
//! - IST4: Double Fault (#DF)
//! - IST5: Machine Check (#MC)
//! - IST6: Debug (#DB)
//! - IST7: static reserve

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::cpu::topology::MAX_CPUS;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Size of an IST stack (16 KiB = 4 pages)
pub const IST_STACK_SIZE: usize = 16 * 1024;

/// Size of an IST guard page (4 KiB = 1 page)
const IST_GUARD_SIZE: usize = 4096;

/// Size of the RSP0 kernel stack (16 KiB)
pub const KERNEL_STACK_SIZE: usize = 16384;

/// Number of IST stacks in the TSS
pub const IST_COUNT: usize = 7;

/// IST index for critical ExoPhoenix IPIs (0xF0/0xF1/0xF2)
pub const IST_EXOPHOENIX_IPI: usize = 0; // IST1

/// IST index for #PF (hardening against corrupted stack)
pub const IST_PAGE_FAULT: usize = 1; // IST2

/// IST index for NMI
pub const IST_NMI: usize = 2; // IST3

/// IST index for Double Fault
pub const IST_DOUBLE_FAULT: usize = 3; // IST4

/// IST index for Machine Check
pub const IST_MACHINE_CHECK: usize = 4; // IST5

/// IST index for Debug (#DB)
pub const IST_DEBUG: usize = 5; // IST6

/// Number of Kernel B IST stacks allocated via early pool.
const GUARDED_IST_STACKS_PER_CPU: usize = 3; // ExoPhoenix + #PF + NMI fallback

/// Slot size: 1 guard page + 1 IST stack.
const GUARDED_IST_SLOT_SIZE: usize = IST_GUARD_SIZE + IST_STACK_SIZE;

/// Total early pool size for all Kernel B IST stacks.
const EARLY_IST_POOL_SIZE: usize = MAX_CPUS * GUARDED_IST_STACKS_PER_CPU * GUARDED_IST_SLOT_SIZE;

// ── TSS Structure ─────────────────────────────────────────────────────────────

/// x86_64 TSS (104 bytes, 16-byte aligned)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct TaskStateSegment {
    _reserved0: u32,
    /// Privilege-level stacks (RSP0 = Ring 3 → Ring 0 entry)
    pub rsp: [u64; 3], // RSP0, RSP1, RSP2 (offsets 0x04..0x1C)
    _reserved1: u32,   // offset 0x1C..0x20
    /// Interrupt Stack Table (7 entries)
    pub ist: [u64; 7], // IST1-IST7 (offsets 0x20..0x58)
    _reserved2: u64,   // offset 0x58..0x60 (8 bytes)
    _reserved3: u32,   // offset 0x60..0x64 (4 bytes)
    _reserved4: u16,   // offset 0x64..0x66 (2 bytes)
    /// Offset of I/O Permission Bitmap (set to TSS size = no IOPB)
    pub iopb_offset: u16, // offset 0x66..0x68 (2 bytes)
}

const _: () = {
    assert!(core::mem::size_of::<TaskStateSegment>() == 104);
    assert!(core::mem::offset_of!(TaskStateSegment, rsp) == 0x04);
    assert!(core::mem::offset_of!(TaskStateSegment, ist) == 0x20);
    assert!(core::mem::offset_of!(TaskStateSegment, iopb_offset) == 0x66);
};

impl TaskStateSegment {
    pub const fn zero() -> Self {
        Self {
            _reserved0: 0,
            rsp: [0u64; 3],
            _reserved1: 0,
            ist: [0u64; 7],
            _reserved2: 0,
            _reserved3: 0,
            _reserved4: 0,
            iopb_offset: core::mem::size_of::<TaskStateSegment>() as u16,
        }
    }
}

// ── Per-CPU IST Stacks ────────────────────────────────────────────────────────

/// IST stacks for a CPU (statically allocated)
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct PerCpuStacks {
    /// Double Fault stack
    df_stack: [u8; IST_STACK_SIZE],
    /// Machine Check stack
    mc_stack: [u8; IST_STACK_SIZE],
    /// Debug stack
    db_stack: [u8; IST_STACK_SIZE],
    /// IST7 stack (reserved)
    ist7_stack: [u8; IST_STACK_SIZE],
}

impl PerCpuStacks {
    const fn zero() -> Self {
        Self {
            df_stack: [0u8; IST_STACK_SIZE],
            mc_stack: [0u8; IST_STACK_SIZE],
            db_stack: [0u8; IST_STACK_SIZE],
            ist7_stack: [0u8; IST_STACK_SIZE],
        }
    }
}

/// Static per-CPU TSS (limit: MAX_CPUS)
static mut CPU_TSS: [TaskStateSegment; MAX_CPUS] = [TaskStateSegment::zero(); MAX_CPUS];
static mut CPU_STACKS: [PerCpuStacks; MAX_CPUS] = [PerCpuStacks::zero(); MAX_CPUS];

#[repr(C, align(4096))]
struct EarlyIstPool {
    bytes: [u8; EARLY_IST_POOL_SIZE],
}

static mut EARLY_IST_POOL: EarlyIstPool = EarlyIstPool {
    bytes: [0u8; EARLY_IST_POOL_SIZE],
};
static EARLY_IST_NEXT: AtomicUsize = AtomicUsize::new(0);

static TSS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Allocates a guarded IST stack from the early pool (heapless).
///
/// Slot layout: [guard page 4 KiB][stack 16 KiB]
/// Guard page is reserved and never used as a stack.
fn alloc_guarded_stack(cpu_id: usize, purpose: &'static str) -> u64 {
    let slot = EARLY_IST_NEXT.fetch_add(GUARDED_IST_SLOT_SIZE, Ordering::AcqRel);
    assert!(
        slot + GUARDED_IST_SLOT_SIZE <= EARLY_IST_POOL_SIZE,
        "TSS: EARLY_IST_POOL épuisé (cpu={}, purpose={})",
        cpu_id,
        purpose,
    );

    // SAFETY: `slot` is unique (fetch_add) and bounded by assert above.
    // The pool is static, 4 KiB aligned, and lives for the entire kernel lifetime.
    let stack_base = unsafe {
        let base = core::ptr::addr_of_mut!(EARLY_IST_POOL.bytes) as *mut u8;
        base.add(slot + IST_GUARD_SIZE) as u64
    };

    stack_base + IST_STACK_SIZE as u64
}

// ── Initialization ───────────────────────────────────────────────────────────

/// Initializes TSS for the current CPU
///
/// `cpu_id`: logical CPU identifier (0 for BSP)
/// `kernel_rsp0`: top address of kernel stack (RSP0)
///
/// # SAFETY
/// Must be called from CPU `cpu_id` after GDT init.
/// The TSS segment must have been loaded into the GDT before `ltr`.
pub fn init_tss_for_cpu(cpu_id: usize, kernel_rsp0: u64) {
    assert!(cpu_id < MAX_CPUS, "TSS: cpu_id hors bornes");

    // SAFETY: cpu_id is unique per CPU; addr_of_mut!/addr_of! avoid creating
    // a reference to the entire mutable static.
    let tss = unsafe { &mut *core::ptr::addr_of_mut!(CPU_TSS[cpu_id]) };
    // SAFETY: CPU_STACKS[cpu_id] is statically initialized and read only here.
    let stacks = unsafe { &*core::ptr::addr_of!(CPU_STACKS[cpu_id]) };

    // RSP0: kernel stack for exceptions originating from userspace
    tss.rsp[0] = kernel_rsp0;

    // Kernel B (Phase 3.1): 3 critical ISTs allocated from an early pool,
    // heapless, with logical guard page (dedicated unused slot).
    let exophoenix_top = alloc_guarded_stack(cpu_id, "exophoenix");
    let page_fault_top = alloc_guarded_stack(cpu_id, "page_fault");
    let nmi_fallback_top = alloc_guarded_stack(cpu_id, "nmi_fallback");

    // IST1–7: top of dedicated stacks (stack grows downwards)
    let df_top = stacks.df_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
    let mc_top = stacks.mc_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
    let db_top = stacks.db_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
    let ist7_top = stacks.ist7_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;

    tss.ist[IST_EXOPHOENIX_IPI] = exophoenix_top;
    tss.ist[IST_PAGE_FAULT] = page_fault_top;
    tss.ist[IST_NMI] = nmi_fallback_top;
    tss.ist[IST_DOUBLE_FAULT] = df_top;
    tss.ist[IST_MACHINE_CHECK] = mc_top;
    tss.ist[IST_DEBUG] = db_top;
    tss.ist[6] = ist7_top;

    // IOPB: pointed beyond TSS → I/O access forbidden from Ring 3
    tss.iopb_offset = core::mem::size_of::<TaskStateSegment>() as u16;

    TSS_INITIALIZED.store(true, Ordering::Release);
}

/// Returns a pointer to the TSS of CPU `cpu_id`
///
/// # SAFETY
/// `cpu_id` must be < MAX_CPUS and the TSS must have been initialized.
pub unsafe fn tss_ptr(cpu_id: usize) -> *const TaskStateSegment {
    // SAFETY: delegated to caller
    unsafe { core::ptr::addr_of!(CPU_TSS[cpu_id]) }
}

/// Returns a mutable pointer to the TSS of CPU `cpu_id`
///
/// # SAFETY
/// Same as `tss_ptr`. Furthermore, caller guarantees absence of data races.
pub unsafe fn tss_ptr_mut(cpu_id: usize) -> *mut TaskStateSegment {
    // SAFETY: delegated to caller
    unsafe { core::ptr::addr_of_mut!(CPU_TSS[cpu_id]) }
}

/// Updates RSP0 in current TSS (after context switch)
///
/// Must be called on each entry into kernel from a new thread.
///
/// # SAFETY
/// `cpu_id` must be the current CPU. `rsp0` points to the top of kernel
/// stack of the incoming thread.
#[inline(always)]
pub unsafe fn update_rsp0(cpu_id: usize, rsp0: u64) {
    // SAFETY: delegated to caller — atomic update of rsp[0] field
    unsafe {
        CPU_TSS[cpu_id].rsp[0] = rsp0;
    }
}

/// Loads TSS into TR register via LTR
///
/// # SAFETY
/// TSS selector must be valid in the current GDT.
/// Must be called after `init_tss_for_cpu()`.
#[inline]
pub unsafe fn load_tss(tss_selector: u16) {
    // SAFETY: valid selector guaranteed by caller
    unsafe {
        core::arch::asm!(
            "ltr {sel:x}",
            sel = in(reg) tss_selector,
            options(nostack, nomem)
        );
    }
}

/// Returns `true` if TSS has been initialized (at least for cpu_id 0)
#[inline(always)]
pub fn tss_ready() -> bool {
    TSS_INITIALIZED.load(Ordering::Relaxed)
}

// ── IST validation ────────────────────────────────────────────────────────────

/// Verifies that all IST stacks are properly aligned
///
/// Returns `Err(&str)` if a stack is misaligned.
pub fn validate_ist_alignment(cpu_id: usize) -> Result<(), &'static str> {
    if cpu_id >= MAX_CPUS {
        return Err("cpu_id hors bornes");
    }
    // SAFETY: cpu_id < MAX_CPUS verified above; CPU_TSS is a static array
    // whose cpu_id index is read-only here — no data race possible.
    // SAFETY: Non-concurrent read of a TSS initialized by this CPU.
    // Uses read_unaligned because TaskStateSegment is repr(packed).
    let tss_ptr = unsafe { core::ptr::addr_of!(CPU_TSS[cpu_id]) };
    let ist_copy: [u64; 7] =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*tss_ptr).ist)) };

    for (i, &ist_addr) in ist_copy.iter().enumerate() {
        if ist_addr == 0 {
            continue;
        }
        if ist_addr % 16 != 0 {
            return Err("IST stack non aligné sur 16 bytes");
        }
        let _ = i;
    }
    Ok(())
}
