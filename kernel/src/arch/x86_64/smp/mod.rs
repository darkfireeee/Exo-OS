//! SMP (Symmetric Multi-Processing) Support
//!
//! Phase 4D: Multi-core initialization and management
//!
//! Handles:
//! - AP (Application Processor) initialization
//! - Per-CPU data structures
//! - Inter-processor interrupts (IPI)
//! - CPU topology detection

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
    /// CPU in error state
    Error = 4,
}

/// Per-CPU information
#[repr(C, align(64))]  // Cache-line aligned
pub struct CpuInfo {
    /// CPU ID (APIC ID)
    pub id: u8,
    /// CPU state
    pub state: AtomicU8,
    /// Is this the BSP (Bootstrap Processor)?
    pub is_bsp: bool,
    /// APIC ID
    pub apic_id: u8,
    /// Local APIC base address
    pub apic_base: usize,
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
            is_bsp: false,
            apic_id: 0,
            apic_base: 0,
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

/// Initialize SMP (Phase 4D - TODO)
pub fn init() -> Result<(), &'static str> {
    crate::logger::info("[SMP] Detecting CPUs...");
    
    // TODO: Phase 4D
    // 1. Parse ACPI MADT table to get CPU count and APIC IDs
    // 2. Detect BSP (current CPU)
    // 3. Initialize BSP APIC
    // 4. For each AP:
    //    a. Send INIT IPI
    //    b. Wait 10ms
    //    c. Send SIPI IPI with startup code address
    //    d. Wait for AP to set its state to Online
    // 5. Setup per-CPU run queues in scheduler
    
    // For now, just detect BSP
    let cpu_count = detect_cpu_count();
    SMP_SYSTEM.cpu_count.store(cpu_count, Ordering::Release);
    
    // Mark BSP as online
    SMP_SYSTEM.cpus[0].set_state(CpuState::Online);
    SMP_SYSTEM.cpus[0].is_bsp = true;
    SMP_SYSTEM.online_count.store(1, Ordering::Release);
    SMP_SYSTEM.initialized.store(true, Ordering::Release);
    
    crate::logger::info(&alloc::format!("[SMP] Detected {} CPUs (1 online)", cpu_count));
    
    Ok(())
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
