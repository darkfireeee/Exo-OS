//! Symmetric Multi-Processing (SMP)
//! 
//! Boot and manage multiple CPU cores using INIT-SIPI-SIPI sequence.
//! 
//! ## Boot Sequence
//! 1. BSP parses MADT to discover APs
//! 2. BSP copies trampoline code to low memory (< 1MB)
//! 3. BSP sends INIT IPI to each AP
//! 4. BSP sends SIPI (Startup IPI) with trampoline address
//! 5. AP executes trampoline: real mode → protected mode → long mode
//! 6. AP calls ap_entry() to complete initialization

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};

/// Maximum number of supported CPUs
pub const MAX_CPUS: usize = 256;

/// AP boot status
static AP_COUNT: AtomicU32 = AtomicU32::new(0);
static BSP_APIC_ID: AtomicU32 = AtomicU32::new(0);
static AP_BOOT_COMPLETE: AtomicBool = AtomicBool::new(false);

/// Per-CPU data (indexed by APIC ID)
static mut CPU_DATA: [CpuData; MAX_CPUS] = [CpuData::new(); MAX_CPUS];

/// Trampoline address (must be < 1MB, page-aligned)
const TRAMPOLINE_ADDR: u64 = 0x8000;

/// x2APIC MSRs
const IA32_APIC_BASE: u32 = 0x1B;
const X2APIC_ID: u32 = 0x802;
const X2APIC_ICR: u32 = 0x830;
const X2APIC_SIVR: u32 = 0x80F;

/// ICR delivery modes
const ICR_INIT: u64 = 0x500;
const ICR_STARTUP: u64 = 0x600;
const ICR_LEVEL_ASSERT: u64 = 0x4000;
const ICR_LEVEL_DEASSERT: u64 = 0x0000;

/// Per-CPU data structure
#[derive(Debug, Clone, Copy)]
pub struct CpuData {
    pub apic_id: u32,
    pub acpi_id: u32,
    pub is_bsp: bool,
    pub is_online: bool,
    pub stack_top: u64,
}

impl CpuData {
    pub const fn new() -> Self {
        Self {
            apic_id: 0,
            acpi_id: 0,
            is_bsp: false,
            is_online: false,
            stack_top: 0,
        }
    }
}

/// Initialize SMP subsystem
pub fn init() {
    // Get BSP's APIC ID
    let bsp_id = get_apic_id();
    BSP_APIC_ID.store(bsp_id, Ordering::SeqCst);
    
    // Mark BSP as online
    unsafe {
        if (bsp_id as usize) < MAX_CPUS {
            CPU_DATA[bsp_id as usize] = CpuData {
                apic_id: bsp_id,
                acpi_id: 0,
                is_bsp: true,
                is_online: true,
                stack_top: 0, // BSP uses boot stack
            };
        }
    }
    
    log::info!("SMP: BSP APIC ID = {}", bsp_id);
}

/// Boot Application Processors
pub fn boot_aps() {
    // Get MADT info from ACPI
    let madt_info = match crate::acpi::get_madt_info() {
        Some(info) => info,
        None => {
            log::warn!("SMP: MADT not available, running single-core");
            return;
        }
    };
    
    let bsp_id = BSP_APIC_ID.load(Ordering::SeqCst);
    let ap_list: Vec<_> = madt_info.cpus.iter()
        .filter(|cpu| cpu.enabled && cpu.apic_id != bsp_id)
        .collect();
    
    if ap_list.is_empty() {
        log::info!("SMP: No APs to boot (single-core system)");
        return;
    }
    
    log::info!("SMP: Found {} APs to boot", ap_list.len());
    
    // Setup trampoline
    setup_trampoline();
    
    // Boot each AP
    for ap in &ap_list {
        boot_ap(ap.apic_id);
    }
    
    // Wait for all APs to boot (timeout 1 second)
    let expected = ap_list.len() as u32;
    let mut timeout = 1000;
    while AP_COUNT.load(Ordering::SeqCst) < expected && timeout > 0 {
        // Busy wait ~1ms
        for _ in 0..100000 {
            core::hint::spin_loop();
        }
        timeout -= 1;
    }
    
    let booted = AP_COUNT.load(Ordering::SeqCst);
    if booted == expected {
        log::info!("SMP: All {} APs booted successfully", booted);
    } else {
        log::warn!("SMP: Only {}/{} APs booted", booted, expected);
    }
    
    AP_BOOT_COMPLETE.store(true, Ordering::SeqCst);
}

/// Setup trampoline code in low memory
fn setup_trampoline() {
    // The trampoline is assembly code that:
    // 1. Starts in 16-bit real mode
    // 2. Enables protected mode
    // 3. Enables long mode
    // 4. Jumps to ap_entry
    
    unsafe {
        // Copy trampoline to low memory
        let trampoline_start = AP_TRAMPOLINE.as_ptr();
        let trampoline_size = AP_TRAMPOLINE.len();
        
        let dest = TRAMPOLINE_ADDR as *mut u8;
        core::ptr::copy_nonoverlapping(
            trampoline_start,
            dest,
            trampoline_size
        );
        
        // Fill in the data area at the end of the trampoline
        // Offsets are relative to trampoline start
        let data_offset = trampoline_size - 24; // 3 x u64 at end
        
        // CR3 - use current page table
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        let cr3_ptr = (TRAMPOLINE_ADDR + data_offset as u64) as *mut u64;
        core::ptr::write_volatile(cr3_ptr, cr3);
        
        // Stack - allocate per-AP stack
        let stack_ptr = (TRAMPOLINE_ADDR + data_offset as u64 + 8) as *mut u64;
        // For now, use a fixed stack address (should allocate properly)
        let ap_stack = 0x200000u64; // 2MB - temporary
        core::ptr::write_volatile(stack_ptr, ap_stack);
        
        // Entry point
        let entry_ptr = (TRAMPOLINE_ADDR + data_offset as u64 + 16) as *mut u64;
        core::ptr::write_volatile(entry_ptr, ap_entry as u64);
    }
    
    log::debug!("SMP: Trampoline installed at {:#x}", TRAMPOLINE_ADDR);
}

/// AP trampoline binary (assembled from ap_trampoline.asm)
/// This is hand-assembled machine code for the 16-bit → 64-bit transition
static AP_TRAMPOLINE: &[u8] = &[
    // 16-bit real mode code at 0x8000
    0xFA,                           // cli
    0xFC,                           // cld
    0x31, 0xC0,                     // xor ax, ax
    0x8E, 0xD8,                     // mov ds, ax
    0x8E, 0xC0,                     // mov es, ax
    0x8E, 0xD0,                     // mov ss, ax
    0xBC, 0x00, 0x70,               // mov sp, 0x7000
    
    // Enable A20
    0xE4, 0x92,                     // in al, 0x92
    0x0C, 0x02,                     // or al, 2
    0xE6, 0x92,                     // out 0x92, al
    
    // Load GDT
    0x0F, 0x01, 0x16,               // lgdt [gdt_ptr]
    0x90, 0x80,                     // offset to gdt_ptr (0x8090)
    
    // Enable protected mode
    0x0F, 0x20, 0xC0,               // mov eax, cr0
    0x0C, 0x01,                     // or al, 1
    0x0F, 0x22, 0xC0,               // mov cr0, eax
    
    // Far jump to 32-bit code (0x08:0x8030)
    0x66, 0xEA,                     // jmp far
    0x30, 0x80, 0x00, 0x00,         // offset 0x00008030
    0x08, 0x00,                     // selector 0x08
    
    // 32-bit protected mode (offset 0x30)
    0x66, 0xB8, 0x10, 0x00,         // mov ax, 0x10
    0x8E, 0xD8,                     // mov ds, ax
    0x8E, 0xC0,                     // mov es, ax
    0x8E, 0xD0,                     // mov ss, ax
    
    // Enable PAE
    0x0F, 0x20, 0xE0,               // mov eax, cr4
    0x0D, 0x20, 0x00, 0x00, 0x00,   // or eax, 0x20
    0x0F, 0x22, 0xE0,               // mov cr4, eax
    
    // Load CR3 from data area (offset 0xA0)
    0x8B, 0x05,                     // mov eax, [data]
    0xA0, 0x80, 0x00, 0x00,         // address 0x80A0
    0x0F, 0x22, 0xD8,               // mov cr3, eax
    
    // Enable long mode
    0xB9, 0x80, 0x00, 0x00, 0xC0,   // mov ecx, 0xC0000080
    0x0F, 0x32,                     // rdmsr
    0x0D, 0x00, 0x01, 0x00, 0x00,   // or eax, 0x100
    0x0F, 0x30,                     // wrmsr
    
    // Enable paging
    0x0F, 0x20, 0xC0,               // mov eax, cr0
    0x0D, 0x00, 0x00, 0x00, 0x80,   // or eax, 0x80000000
    0x0F, 0x22, 0xC0,               // mov cr0, eax
    
    // Far jump to 64-bit code (0x18:0x8078)
    0xEA,                           // jmp far
    0x78, 0x80, 0x00, 0x00,         // offset 0x00008078
    0x18, 0x00,                     // selector 0x18
    
    // 64-bit long mode (offset 0x78)
    0x48, 0xC7, 0xC0, 0x20, 0x00, 0x00, 0x00, // mov rax, 0x20
    0x8E, 0xD8,                     // mov ds, ax
    0x8E, 0xC0,                     // mov es, ax
    0x8E, 0xD0,                     // mov ss, ax
    
    // Load stack from data area
    0x48, 0x8B, 0x24, 0x25,         // mov rsp, [data+8]
    0xA8, 0x80, 0x00, 0x00,         // address 0x80A8
    
    // Get APIC ID (x2APIC)
    0xB9, 0x02, 0x08, 0x00, 0x00,   // mov ecx, 0x802
    0x0F, 0x32,                     // rdmsr
    0x89, 0xC7,                     // mov edi, eax
    
    // Load entry point and call
    0x48, 0x8B, 0x04, 0x25,         // mov rax, [data+16]
    0xB0, 0x80, 0x00, 0x00,         // address 0x80B0
    0xFF, 0xD0,                     // call rax
    
    // Halt if returned
    0xF4,                           // hlt
    0xEB, 0xFD,                     // jmp -3
    
    // Padding to GDT (offset 0xA0)
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90,
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90,
    
    // Data area (offset 0xA0) - filled by BSP
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // CR3
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Stack
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Entry
    
    // GDT (offset 0xB8)
    // Null descriptor
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // 32-bit code (0x08)
    0xFF, 0xFF, 0x00, 0x00, 0x00, 0x9A, 0xCF, 0x00,
    // 32-bit data (0x10)
    0xFF, 0xFF, 0x00, 0x00, 0x00, 0x92, 0xCF, 0x00,
    // 64-bit code (0x18)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x9A, 0x20, 0x00,
    // 64-bit data (0x20)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x92, 0x00, 0x00,
    
    // GDT pointer (offset 0xE0)
    0x27, 0x00,                     // limit (5*8 - 1 = 39)
    0xB8, 0x80, 0x00, 0x00,         // base 0x80B8
];

/// Boot a single AP using INIT-SIPI-SIPI sequence
fn boot_ap(apic_id: u32) {
    log::debug!("SMP: Booting AP {}", apic_id);
    
    unsafe {
        // Check if x2APIC is enabled
        let apic_base = rdmsr(IA32_APIC_BASE);
        let x2apic = (apic_base & (1 << 10)) != 0;
        
        if x2apic {
            boot_ap_x2apic(apic_id);
        } else {
            boot_ap_xapic(apic_id);
        }
    }
}

/// Boot AP using x2APIC
unsafe fn boot_ap_x2apic(apic_id: u32) {
    // 1. Send INIT IPI
    let icr = ICR_INIT | ICR_LEVEL_ASSERT | ((apic_id as u64) << 32);
    wrmsr(X2APIC_ICR, icr);
    
    // Wait 10ms
    delay_ms(10);
    
    // De-assert INIT
    let icr = ICR_INIT | ICR_LEVEL_DEASSERT | ((apic_id as u64) << 32);
    wrmsr(X2APIC_ICR, icr);
    
    // 2. Send SIPI (twice for reliability)
    let vector = (TRAMPOLINE_ADDR >> 12) as u64; // Page number
    let icr = ICR_STARTUP | vector | ((apic_id as u64) << 32);
    
    wrmsr(X2APIC_ICR, icr);
    delay_us(200);
    
    wrmsr(X2APIC_ICR, icr);
    delay_us(200);
}

/// Boot AP using xAPIC (MMIO)
unsafe fn boot_ap_xapic(_apic_id: u32) {
    // xAPIC requires MMIO at 0xFEE00000
    // For now, just log a warning
    log::warn!("SMP: xAPIC boot not implemented (requires MMIO mapping)");
}

/// AP entry point - called from trampoline
#[no_mangle]
pub extern "C" fn ap_entry(apic_id: u32) {
    // This is called by each AP after trampoline sets up long mode
    
    unsafe {
        // Initialize per-CPU data
        if (apic_id as usize) < MAX_CPUS {
            CPU_DATA[apic_id as usize].is_online = true;
            CPU_DATA[apic_id as usize].apic_id = apic_id;
        }
        
        // Setup GDT (use same as BSP for now)
        // Setup IDT (use same as BSP)
        
        // Enable x2APIC
        let apic_base = rdmsr(IA32_APIC_BASE);
        wrmsr(IA32_APIC_BASE, apic_base | (1 << 10) | (1 << 11));
        
        // Enable APIC
        let sivr = rdmsr(X2APIC_SIVR);
        wrmsr(X2APIC_SIVR, sivr | (1 << 8) | 0xFF);
    }
    
    // Signal that we're online
    AP_COUNT.fetch_add(1, Ordering::SeqCst);
    
    log::info!("SMP: AP {} online", apic_id);
    
    // Enter idle loop (or scheduler)
    ap_idle_loop();
}

/// AP idle loop
fn ap_idle_loop() -> ! {
    loop {
        // Wait for work
        // In a real implementation, this would check a per-CPU run queue
        // and execute threads, or enter a low-power state
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Get current APIC ID
pub fn get_apic_id() -> u32 {
    unsafe {
        let apic_base = rdmsr(IA32_APIC_BASE);
        if (apic_base & (1 << 10)) != 0 {
            // x2APIC mode
            rdmsr(X2APIC_ID) as u32
        } else {
            // Use CPUID fallback
            let cpuid = core::arch::x86_64::__cpuid(1);
            (cpuid.ebx >> 24) as u32
        }
    }
}

/// Get number of online CPUs
pub fn cpu_count() -> u32 {
    1 + AP_COUNT.load(Ordering::Relaxed)
}

/// Get current CPU's ID
pub fn current_cpu() -> u32 {
    get_apic_id()
}

/// Check if current CPU is BSP
pub fn is_bsp() -> bool {
    get_apic_id() == BSP_APIC_ID.load(Ordering::Relaxed)
}

/// Get per-CPU data for current CPU
pub fn get_cpu_data() -> &'static CpuData {
    let id = get_apic_id() as usize;
    unsafe {
        if id < MAX_CPUS {
            &CPU_DATA[id]
        } else {
            &CPU_DATA[0]
        }
    }
}

/// Send IPI to a specific CPU
pub fn send_ipi(target_apic_id: u32, vector: u8) {
    unsafe {
        let apic_base = rdmsr(IA32_APIC_BASE);
        if (apic_base & (1 << 10)) != 0 {
            // x2APIC
            let icr = (vector as u64) | ((target_apic_id as u64) << 32);
            wrmsr(X2APIC_ICR, icr);
        } else {
            log::warn!("send_ipi: xAPIC not implemented");
        }
    }
}

/// Send IPI to all CPUs except self
pub fn send_ipi_all_excluding_self(vector: u8) {
    unsafe {
        let apic_base = rdmsr(IA32_APIC_BASE);
        if (apic_base & (1 << 10)) != 0 {
            // x2APIC - destination shorthand: all excluding self
            let icr = (vector as u64) | (0b11 << 18); // All excluding self
            wrmsr(X2APIC_ICR, icr);
        }
    }
}

// Helper functions

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags)
    );
}

/// Delay approximately N milliseconds
fn delay_ms(ms: u32) {
    for _ in 0..ms {
        delay_us(1000);
    }
}

/// Delay approximately N microseconds
fn delay_us(us: u32) {
    // Simple busy-wait delay
    // ~3 cycles per iteration on modern CPUs
    for _ in 0..(us * 1000) {
        core::hint::spin_loop();
    }
}
