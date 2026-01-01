//! SMP (Symmetric Multi-Processing) Support
//!
//! Phase 4D: Multi-core initialization and management
//!
//! Handles:
//! - AP (Application Processor) initialization
//! - Per-CPU data structures
//! - Inter-processor interrupts (IPI)
//! - CPU topology detection

pub mod bootstrap;

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

/// Maximum supported CPUs
pub const MAX_CPUS: usize = 64;

/// CPU state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CpuState {
    /// CPU not initialized
    NotInitialized = 0,
    /// CPU initializing
    Initializing = 1,
    /// CPU online and running
    Online = 2,
    /// CPU offline
    Offline = 3,
    /// CPU halted
    Halted = 4,
    /// CPU in error state
    Error = 5,
}

/// Per-CPU information
#[repr(C, align(64))]  // Cache-line aligned
pub struct CpuInfo {
    /// CPU ID (APIC ID)
    pub id: u8,
    /// CPU state
    pub state: AtomicU8,
    /// Is this the BSP (Bootstrap Processor)?
    pub is_bsp: core::sync::atomic::AtomicBool,
    /// APIC ID
    pub apic_id: core::sync::atomic::AtomicU8,
    /// Local APIC base address
    pub apic_base: AtomicUsize,
    /// CPU features (CPUID)
    pub features: CpuFeatures,
    /// Number of context switches on this CPU
    pub context_switches: AtomicUsize,
    /// Time spent idle (ns)
    pub idle_time_ns: AtomicUsize,
    /// Time spent busy (ns)
    pub busy_time_ns: AtomicUsize,
}

impl CpuInfo {
    pub const fn new(id: u8) -> Self {
        Self {
            id,
            state: AtomicU8::new(CpuState::NotInitialized as u8),
            is_bsp: core::sync::atomic::AtomicBool::new(false),
            apic_id: core::sync::atomic::AtomicU8::new(0),
            apic_base: AtomicUsize::new(0),
            features: CpuFeatures::empty(),
            context_switches: AtomicUsize::new(0),
            idle_time_ns: AtomicUsize::new(0),
            busy_time_ns: AtomicUsize::new(0),
        }
    }
    
    pub fn state(&self) -> CpuState {
        match self.state.load(Ordering::Acquire) {
            0 => CpuState::NotInitialized,
            1 => CpuState::Initializing,
            2 => CpuState::Online,
            3 => CpuState::Offline,
            _ => CpuState::Error,
        }
    }
    
    pub fn set_state(&self, state: CpuState) {
        self.state.store(state as u8, Ordering::Release);
    }
    
    pub fn is_online(&self) -> bool {
        self.state() == CpuState::Online
    }
}

/// CPU features from CPUID
#[derive(Debug, Clone, Copy)]
pub struct CpuFeatures {
    pub vendor: [u8; 12],
    pub brand: [u8; 48],
    pub has_sse: bool,
    pub has_sse2: bool,
    pub has_sse3: bool,
    pub has_ssse3: bool,
    pub has_sse4_1: bool,
    pub has_sse4_2: bool,
    pub has_avx: bool,
    pub has_avx2: bool,
    pub has_fma: bool,
    pub has_aes: bool,
    pub has_rdrand: bool,
    pub has_rdseed: bool,
    pub has_bmi1: bool,
    pub has_bmi2: bool,
    pub has_popcnt: bool,
    pub has_tsc: bool,
    pub has_invariant_tsc: bool,
    pub has_apic: bool,
    pub has_x2apic: bool,
}

impl CpuFeatures {
    pub const fn empty() -> Self {
        Self {
            vendor: [0; 12],
            brand: [0; 48],
            has_sse: false,
            has_sse2: false,
            has_sse3: false,
            has_ssse3: false,
            has_sse4_1: false,
            has_sse4_2: false,
            has_avx: false,
            has_avx2: false,
            has_fma: false,
            has_aes: false,
            has_rdrand: false,
            has_rdseed: false,
            has_bmi1: false,
            has_bmi2: false,
            has_popcnt: false,
            has_tsc: false,
            has_invariant_tsc: false,
            has_apic: false,
            has_x2apic: false,
        }
    }
}

/// SMP system state
pub struct SmpSystem {
    /// Number of CPUs detected
    cpu_count: AtomicUsize,
    /// Number of online CPUs
    online_count: AtomicUsize,
    /// BSP CPU ID
    bsp_id: AtomicU8,
    /// Per-CPU information
    cpus: [CpuInfo; MAX_CPUS],
    /// SMP initialized
    initialized: AtomicBool,
}

impl SmpSystem {
    pub const fn new() -> Self {
        const CPU_INIT: CpuInfo = CpuInfo::new(0);
        Self {
            cpu_count: AtomicUsize::new(1),  // BSP is always present
            online_count: AtomicUsize::new(0),
            bsp_id: AtomicU8::new(0),
            cpus: [CPU_INIT; MAX_CPUS],
            initialized: AtomicBool::new(false),
        }
    }
    
    pub fn cpu_count(&self) -> usize {
        self.cpu_count.load(Ordering::Acquire)
    }
    
    pub fn online_count(&self) -> usize {
        self.online_count.load(Ordering::Acquire)
    }
    
    pub fn bsp_id(&self) -> u8 {
        self.bsp_id.load(Ordering::Acquire)
    }
    
    pub fn cpu(&self, id: usize) -> Option<&CpuInfo> {
        if id < MAX_CPUS {
            Some(&self.cpus[id])
        } else {
            None
        }
    }
    
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}

/// Global SMP system
pub static SMP_SYSTEM: SmpSystem = SmpSystem::new();

/// AP startup function (called from trampoline.asm)
/// 
/// CRITICAL: Runs with interrupts DISABLED - uses port 0xE9 only for debug
/// This avoids serial port lock contention with BSP
#[no_mangle]
pub extern "C" fn ap_startup(cpu_id: u64) -> ! {
    use crate::arch::x86_64::{interrupts, percpu};
    
    // === STAGE 1: Validate CPU ID ===
    if cpu_id as usize >= MAX_CPUS {
        unsafe {
            loop {
                core::arch::asm!("cli; hlt");
            }
        }
    }
    
    // === STAGE 2: Initialize FPU/SSE/AVX ===
    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !(1 << 2); // Clear EM
        cr0 |= 1 << 1;    // Set MP
        core::arch::asm!("mov cr0, {}", in(reg) cr0);
        
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= (1 << 9) | (1 << 10); // OSFXSR | OSXMMEXCPT
        
        // Enable OSXSAVE for AVX if supported
        let cpuid = core::arch::x86_64::__cpuid(1);
        if (cpuid.ecx & (1 << 26)) != 0 { // XSAVE supported
            cr4 |= 1 << 18; // Set CR4.OSXSAVE
        }
        core::arch::asm!("mov cr4, {}", in(reg) cr4);
        
        // Initialize XCR0 for AVX if OSXSAVE is set
        if (cr4 & (1 << 18)) != 0 {
            core::arch::asm!(
                "xor ecx, ecx",      // XCR0 register
                "mov eax, 0x7",      // X87 | SSE | AVX
                "xor edx, edx",      // High 32 bits = 0
                "xsetbv",            // Set XCR0
                out("eax") _,
                out("edx") _,
                out("ecx") _,
            );
        }
    }
    
    // === STAGE 3: Mark as initializing (no logging) ===
    if let Some(cpu) = SMP_SYSTEM.cpu(cpu_id as usize) {
        cpu.set_state(CpuState::Initializing);
    }
    
    // === STAGE 4: Initialize Local APIC ===
    if let Some(local_apic) = interrupts::apic::LOCAL_APIC.get() {
        let mut apic = local_apic.lock();
        apic.init();
        let id = apic.get_id();
        drop(apic);
        
        for i in 0..MAX_CPUS {
            if let Some(cpu) = SMP_SYSTEM.cpu(i) {
                if cpu.apic_id.load(Ordering::Acquire) == id as u8 {
                    cpu.apic_base.store(0xFEE00000, Ordering::Release);
                    break;
                }
            }
        }
    } else {
        unsafe {
            loop {
                core::arch::asm!("cli; hlt");
            }
        }
    };
    
    // === STAGE 5: Load IDT ===
    unsafe {
        let (idt_base, idt_limit) = crate::arch::x86_64::idt::get_idt_info();
        
        // Create IDTR structure
        #[repr(C, packed)]
        struct IdtPointer {
            limit: u16,
            base: u64,
        }
        
        let idtr = IdtPointer {
            limit: idt_limit,
            base: idt_base,
        };
        
        // Load IDT
        core::arch::asm!("lidt [{0}]", in(reg) &idtr, options(readonly, nostack, preserves_flags));
    }
    
    // === STAGE 6: Setup per-CPU data ===
    percpu::init(cpu_id as u32);
    
    // === STAGE 7: Configure APIC Timer ===
    interrupts::apic::setup_timer(32);
    
    // === STAGE 8: Mark CPU as online ===
    if let Some(cpu) = SMP_SYSTEM.cpu(cpu_id as usize) {
        cpu.set_state(CpuState::Online);
        cpu.context_switches.store(0, Ordering::Release);
        cpu.idle_time_ns.store(0, Ordering::Release);
        cpu.busy_time_ns.store(0, Ordering::Release);
    }
    
    let _online_count = SMP_SYSTEM.online_count.fetch_add(1, Ordering::AcqRel) + 1;
    
    // === STAGE 9: Send success marker to port 0xE9 ===
    unsafe {
        // Output "AP<n>OK\n" to debug console
        core::arch::asm!("out 0xE9, al", in("al") b'A');
        core::arch::asm!("out 0xE9, al", in("al") b'P');
        core::arch::asm!("out 0xE9, al", in("al") b'0' + (cpu_id as u8));
        core::arch::asm!("out 0xE9, al", in("al") b'O');
        core::arch::asm!("out 0xE9, al", in("al") b'K');
        core::arch::asm!("out 0xE9, al", in("al") b'\n');
    }
    
    // === STAGE 10: Idle loop (interrupts disabled) ===
    
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
        
        if let Some(cpu) = SMP_SYSTEM.cpu(cpu_id as usize) {
            cpu.idle_time_ns.fetch_add(1000000, Ordering::Relaxed);
        }
    }
}

/// Bootstrap Application Processors using already-parsed ACPI info
///
/// This function orchestrates the entire AP boot sequence:
/// 1. Parse MADT for CPU topology
/// 2. Setup trampoline code and data structures
/// 3. Send INIT-SIPI-SIPI sequence to each AP
/// 4. Wait for APs to come online
/// 5. Handle errors and timeouts gracefully
///
/// Returns: Ok(number_of_online_CPUs) or Err(description)
pub fn bootstrap_aps(acpi_info: &crate::arch::x86_64::acpi::AcpiInfo) -> Result<usize, &'static str> {
    use crate::arch::x86_64::interrupts::ipi;
    use crate::arch::x86_64::acpi::madt;
    
    log::info!("═══════════════════════════════════════");
    log::info!("  SMP Bootstrap Sequence");
    log::info!("═══════════════════════════════════════");
    
    // === PHASE 1: Detect CPUs from MADT ===
    let madt_info = madt::parse_madt().map_err(|e| {
        log::error!("Failed to parse MADT: {}", e);
        e
    })?;
    
    let cpu_count = madt_info.cpu_count;
    if cpu_count == 0 {
        return Err("No CPUs detected in MADT");
    }
    if cpu_count > MAX_CPUS {
        log::warn!("MADT reports {} CPUs, but MAX_CPUS is {}", cpu_count, MAX_CPUS);
    }
    
    SMP_SYSTEM.cpu_count.store(cpu_count.min(MAX_CPUS), Ordering::Release);
    
    log::info!("✓ Detected {} CPUs from MADT", cpu_count);
    log::info!("  Local APIC base: {:#x}", madt_info.local_apic_address);
    
    // === PHASE 2: Initialize CPU table ===
    for (i, &apic_id) in madt_info.apic_ids.iter().enumerate() {
        if i >= MAX_CPUS {
            break;
        }
        
        let cpu = &SMP_SYSTEM.cpus[i];
        cpu.apic_id.store(apic_id as u8, Ordering::Release);
        cpu.apic_base.store(madt_info.local_apic_address as usize, Ordering::Release);
        
        log::debug!("  CPU {}: APIC ID {}", i, apic_id);
    }
    
    // === PHASE 3: Identify BSP (Bootstrap Processor) ===
    let bsp_apic_id = madt_info.apic_ids[0];
    SMP_SYSTEM.cpus[0].set_state(CpuState::Online);
    SMP_SYSTEM.cpus[0].is_bsp.store(true, Ordering::Release);
    SMP_SYSTEM.bsp_id.store(0, Ordering::Release);
    SMP_SYSTEM.online_count.store(1, Ordering::Release);
    
    log::info!("✓ BSP identified: CPU 0 (APIC ID {})", bsp_apic_id);
    
    if cpu_count == 1 {
        log::info!("Single-CPU system, SMP bootstrap complete");
        return Ok(1);
    }
    
    // === PHASE 4: Bootstrap APs ===
    log::info!("Starting {} Application Processors...", cpu_count - 1);
    
    let mut successful_boots = 0;
    let mut failed_boots = 0;
    
    for i in 1..cpu_count.min(MAX_CPUS) {
        let apic_id = madt_info.apic_ids[i];
        
        log::info!("─────────────────────────────────────");
        log::info!("Booting AP {} (APIC ID {})...", i, apic_id);
        
        // Try to boot this AP with retry logic
        let result = boot_single_ap(i, apic_id);
        
        match result {
            Ok(()) => {
                successful_boots += 1;
                log::info!("✅ AP {} online!", i);
            }
            Err(e) => {
                failed_boots += 1;
                log::error!("❌ AP {} failed: {}", i, e);
                SMP_SYSTEM.cpus[i].set_state(CpuState::Error);
            }
        }
    }
    
    // === PHASE 5: Summary ===
    log::info!("═══════════════════════════════════════");
    log::info!("  SMP Bootstrap Complete");
    log::info!("═══════════════════════════════════════");
    log::info!("  Total CPUs:    {}", cpu_count);
    log::info!("  Online:        {}", successful_boots + 1); // +1 for BSP
    log::info!("  Failed:        {}", failed_boots);
    log::info!("═══════════════════════════════════════");
    
    if failed_boots > 0 {
        log::warn!("⚠️  {} AP(s) failed to start - continuing with {} CPU(s)", 
                   failed_boots, successful_boots + 1);
    }
    
    Ok(successful_boots + 1)
}

/// Boot a single AP with retry logic and timeout
fn boot_single_ap(cpu_id: usize, apic_id: u32) -> Result<(), &'static str> {
    use crate::arch::x86_64::interrupts::ipi;
    
    const MAX_RETRIES: usize = 2;
    const BOOT_TIMEOUT_MS: u64 = 2000; // 2 seconds per attempt
    
    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            log::info!("  Retry {}/{} for AP {}...", attempt, MAX_RETRIES - 1, cpu_id);
        }
        
        // === Step 1: Allocate resources ===
        let stack_top = bootstrap::allocate_ap_stack(cpu_id)
            .map_err(|_| "Failed to allocate stack")?;
        
        // === Step 2: Setup trampoline ===
        let vector = bootstrap::setup_trampoline(cpu_id, stack_top, ap_startup as u64)
            .map_err(|_| "Failed to setup trampoline")?;
        
        log::debug!("  Trampoline: vector={:#x}, stack={:#x}", vector, stack_top);
        
        // === Step 3: Send INIT IPI ===
        ipi::send_init_ipi(apic_id)
            .map_err(|_| "INIT IPI failed")?;
        log::debug!("  INIT IPI sent");
        
        // === Step 4: Wait INIT de-assert delay (10ms per Intel spec) ===
        crate::arch::x86_64::pit::sleep_ms(10);
        
        // === Step 5: Send 1st SIPI ===
        ipi::send_startup_ipi(apic_id, vector)
            .map_err(|_| "1st SIPI failed")?;
        log::debug!("  1st SIPI sent");
        
        // === Step 6: Wait 200us (Intel spec minimum) ===
        crate::arch::x86_64::pit::sleep_us(200);
        
        // === Step 7: Send 2nd SIPI ===
        ipi::send_startup_ipi(apic_id, vector)
            .map_err(|_| "2nd SIPI failed")?;
        log::debug!("  2nd SIPI sent");
        
        // === Step 8: Wait for AP to come online ===
        let wait_step_ms = 10;
        let max_iterations = BOOT_TIMEOUT_MS / wait_step_ms;
        
        for iteration in 0..max_iterations {
            if SMP_SYSTEM.cpus[cpu_id].is_online() {
                log::debug!("  AP online after {}ms", iteration * wait_step_ms);
                return Ok(());
            }
            crate::arch::x86_64::pit::sleep_ms(wait_step_ms);
        }
        
        // Timeout on this attempt
        log::warn!("  Attempt {} timed out after {}ms", attempt + 1, BOOT_TIMEOUT_MS);
    }
    
    Err("All boot attempts failed")
}

/// Initialize SMP (Phase 2 - Full Implementation)
pub fn init() -> Result<(), &'static str> {
    use crate::arch::x86_64::{acpi, interrupts::ipi};
    use core::time::Duration;
    
    log::info!("Initializing SMP...");
    
    // 1. Initialize ACPI
    acpi::init()?;
    
    // 2. Parse MADT to get CPU information
    let madt_info = acpi::madt::parse_madt()?;
    
    let cpu_count = madt_info.cpu_count;
    SMP_SYSTEM.cpu_count.store(cpu_count, Ordering::Release);
    
    log::info!("Detected {} CPUs from MADT", cpu_count);
    
    // 3. Setup CPUs in SMP_SYSTEM
    for (i, &apic_id) in madt_info.apic_ids.iter().enumerate() {
        if i >= MAX_CPUS {
            break;
        }
        
        let cpu = &SMP_SYSTEM.cpus[i];
        cpu.apic_id.store(apic_id as u8, Ordering::Release);
        cpu.apic_base.store(madt_info.local_apic_address as usize, Ordering::Release);
    }
    
    // 4. Detect BSP (current CPU) - it's always APIC ID in cpus[0] typically
    let bsp_apic_id = madt_info.apic_ids[0];
    SMP_SYSTEM.cpus[0].set_state(CpuState::Online);
    SMP_SYSTEM.cpus[0].is_bsp.store(true, Ordering::Release);
    SMP_SYSTEM.bsp_id.store(0, Ordering::Release);
    SMP_SYSTEM.online_count.store(1, Ordering::Release);
    
    log::info!("BSP APIC ID: {}", bsp_apic_id);
    
    // 5. Bootstrap APs (Application Processors)
    for i in 1..cpu_count {
        if i >= MAX_CPUS {
            break;
        }
        
        let apic_id = madt_info.apic_ids[i];
        
        log::info!("Booting AP {} (APIC ID {})...", i, apic_id);
        
        // Allocate stack for AP
        let stack_top = bootstrap::allocate_ap_stack(i)?;
        
        // Setup trampoline
        let vector = bootstrap::setup_trampoline(i, stack_top, ap_startup as u64)?;
        
        // Send INIT IPI
        ipi::send_init_ipi(apic_id);
        
        // Wait 10ms
        crate::arch::x86_64::pit::sleep_ms(10);
        
        // Send SIPI (Startup IPI) with trampoline vector
        ipi::send_startup_ipi(apic_id, vector);
        
        // Wait for AP to come online (timeout after 1 second)
        let mut timeout = 100; // 100 * 10ms = 1 second
        while timeout > 0 {
            if SMP_SYSTEM.cpus[i].is_online() {
                log::info!("AP {} online!", i);
                break;
            }
            crate::arch::x86_64::pit::sleep_ms(10);
            timeout -= 1;
        }
        
        if timeout == 0 {
            log::error!("AP {} failed to start (timeout)", i);
            SMP_SYSTEM.cpus[i].set_state(CpuState::Error);
        }
    }
    
    let online_count = SMP_SYSTEM.online_count.load(Ordering::Acquire);
    log::info!("SMP initialized: {} / {} CPUs online", online_count, cpu_count);
    
    SMP_SYSTEM.initialized.store(true, Ordering::Release);
    
    Ok(())
}

/// Get total CPU count
pub fn get_cpu_count() -> usize {
    SMP_SYSTEM.cpu_count.load(Ordering::Acquire)
}

/// Get online CPU count
pub fn get_online_count() -> usize {
    SMP_SYSTEM.online_count.load(Ordering::Acquire)
}

/// ACPI MADT (Multiple APIC Description Table) structures
mod acpi {
    #[repr(C, packed)]
    pub struct MadtHeader {
        pub signature: [u8; 4],  // "APIC"
        pub length: u32,
        pub revision: u8,
        pub checksum: u8,
        pub oem_id: [u8; 6],
        pub oem_table_id: [u8; 8],
        pub oem_revision: u32,
        pub creator_id: u32,
        pub creator_revision: u32,
        pub local_apic_address: u32,
        pub flags: u32,
    }
    
    #[repr(C, packed)]
    pub struct MadtEntryHeader {
        pub entry_type: u8,
        pub length: u8,
    }
    
    // Entry type 0: Local APIC
    #[repr(C, packed)]
    pub struct LocalApicEntry {
        pub header: MadtEntryHeader,
        pub acpi_processor_id: u8,
        pub apic_id: u8,
        pub flags: u32,  // Bit 0: enabled
    }
}

/// Detect CPU count from ACPI MADT
fn detect_cpu_count() -> usize {
    // TODO Phase 4D: Properly find ACPI RSDP and parse tables
    // For now, we search for MADT in common BIOS areas
    
    const EBDA_START: usize = 0x80000;
    const EBDA_SIZE: usize = 0x20000;
    const BIOS_ROM_START: usize = 0xE0000;
    const BIOS_ROM_SIZE: usize = 0x20000;
    
    // Search for "APIC" signature in EBDA
    if let Some(count) = search_madt_in_range(EBDA_START, EBDA_SIZE) {
        return count;
    }
    
    // Search in BIOS ROM area
    if let Some(count) = search_madt_in_range(BIOS_ROM_START, BIOS_ROM_SIZE) {
        return count;
    }
    
    crate::logger::warn("[SMP] Could not find ACPI MADT, assuming 1 CPU");
    1
}

/// Search for MADT in memory range
fn search_madt_in_range(start: usize, size: usize) -> Option<usize> {
    // TODO Phase 4D: Implement proper ACPI scanning
    // This requires:
    // 1. Find RSDP (Root System Description Pointer)
    // 2. Parse RSDT/XSDT to find MADT
    // 3. Validate checksums
    // 4. Parse MADT entries
    
    // For now, just return None
    // Real implementation will scan memory for "RSD PTR " signature
    None
}

/// Parse MADT and count CPUs (Phase 4D - stub)
fn parse_madt(madt_ptr: *const acpi::MadtHeader) -> usize {
    let mut cpu_count = 0;
    
    unsafe {
        let madt = &*madt_ptr;
        
        // Verify signature
        if &madt.signature != b"APIC" {
            return 0;
        }
        
        // Parse entries
        let entries_start = (madt_ptr as usize + core::mem::size_of::<acpi::MadtHeader>()) as *const u8;
        let entries_end = (madt_ptr as usize + madt.length as usize) as *const u8;
        
        let mut current = entries_start;
        while current < entries_end {
            let entry_header = &*(current as *const acpi::MadtEntryHeader);
            
            match entry_header.entry_type {
                0 => {
                    // Local APIC entry
                    let apic_entry = &*(current as *const acpi::LocalApicEntry);
                    if apic_entry.flags & 1 != 0 {
                        // CPU is enabled
                        cpu_count += 1;
                        
                        if cpu_count < MAX_CPUS {
                            let cpu_info = &SMP_SYSTEM.cpus[cpu_count - 1];
                            // Store APIC ID
                            // Note: Can't modify const, will be fixed in Phase 4D
                        }
                    }
                }
                _ => {
                    // Other entry types (IO APIC, etc.)
                }
            }
            
            current = current.add(entry_header.length as usize);
        }
    }
    
    cpu_count
}

/// Get current CPU ID
pub fn current_cpu_id() -> u8 {
    // TODO: Read APIC ID from Local APIC
    0
}

/// Send IPI to CPU (Phase 4D - TODO)
pub fn send_ipi(cpu_id: u8, vector: u8) {
    // TODO: Write to Local APIC ICR register
    crate::logger::debug(&alloc::format!("[SMP] Sending IPI vector {} to CPU {}", vector, cpu_id));
}

/// IPI vectors
pub mod ipi_vectors {
    /// TLB shootdown IPI
    pub const TLB_SHOOTDOWN: u8 = 0xF0;
    /// Reschedule IPI
    pub const RESCHEDULE: u8 = 0xF1;
    /// Panic/halt IPI
    pub const PANIC: u8 = 0xF2;
}
