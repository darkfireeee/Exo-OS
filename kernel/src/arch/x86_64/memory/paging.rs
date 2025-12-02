//! 4-level paging support

pub const PAGE_SIZE: usize = 4096;
pub const ENTRIES_PER_TABLE: usize = 512;

/// Page table entry
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub fn new() -> Self {
        Self(0)
    }
    
    pub fn is_present(&self) -> bool {
        self.0 & 1 != 0
    }
    
    pub fn set_present(&mut self, present: bool) {
        if present {
            self.0 |= 1;
        } else {
            self.0 &= !1;
        }
    }
    
    pub fn address(&self) -> u64 {
        self.0 & 0x000F_FFFF_FFFF_F000
    }
    
    pub fn set_address(&mut self, addr: u64) {
        self.0 = (self.0 & !0x000F_FFFF_FFFF_F000) | (addr & 0x000F_FFFF_FFFF_F000);
    }
}

/// Page table (512 entries)
#[repr(align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; ENTRIES_PER_TABLE],
}

impl PageTable {
    pub fn new() -> Self {
        Self {
            entries: [PageTableEntry::new(); ENTRIES_PER_TABLE],
        }
    }
}

// Page table flags
const PAGE_PRESENT: u64 = 1 << 0;
const PAGE_WRITE: u64 = 1 << 1;
const PAGE_HUGE: u64 = 1 << 7;        // 2MB page
const PAGE_NO_CACHE: u64 = 1 << 4;    // Disable caching (important for MMIO)
const PAGE_WRITE_THROUGH: u64 = 1 << 3;

/// Map APIC and I/O APIC memory regions
/// These are at 0xFEE00000 (Local APIC) and 0xFEC00000 (I/O APIC)
/// Must be called early during boot before using APIC
pub fn map_apic_regions() {
    crate::logger::early_print("[PAGING] Mapping APIC regions...\n");
    
    unsafe {
        // Get current CR3 (PML4 physical address)
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        let pml4 = cr3 as *mut u64;
        
        // APIC addresses are in the range 0xFEC00000 - 0xFEF00000
        // This is in PML4 index 0, PDP index 3 (covers 0xC0000000 - 0xFFFFFFFF)
        // We need to add entries to map this region
        
        // Calculate indices for 0xFEE00000
        // PML4 index = (addr >> 39) & 0x1FF = 0
        // PDP index = (addr >> 30) & 0x1FF = 3  
        // PD index = (addr >> 21) & 0x1FF = 503 (for 0xFEE00000 with 2MB pages)
        
        let apic_addr: u64 = 0xFEE00000;
        let ioapic_addr: u64 = 0xFEC00000;
        
        // Get PML4[0] -> should already point to PDP from boot.asm
        let pml4_entry = core::ptr::read_volatile(pml4);
        if pml4_entry & PAGE_PRESENT == 0 {
            crate::logger::early_print("[PAGING] ERROR: PML4[0] not present!\n");
            return;
        }
        let pdp = (pml4_entry & 0x000F_FFFF_FFFF_F000) as *mut u64;
        
        // Check if PDP[3] exists (covers 0xC0000000 - 0xFFFFFFFF)
        let pdp_entry = core::ptr::read_volatile(pdp.add(3));
        
        let pd: *mut u64;
        if pdp_entry & PAGE_PRESENT == 0 {
            // Need to allocate a new Page Directory for PDP[3]
            // Use a static buffer (we know we need this at boot)
            static mut PD_FOR_APIC: [u64; 512] = [0; 512];
            pd = PD_FOR_APIC.as_mut_ptr();
            
            // Set PDP[3] to point to our new PD
            let pd_phys = pd as u64;  // Identity mapped
            core::ptr::write_volatile(pdp.add(3), pd_phys | PAGE_PRESENT | PAGE_WRITE);
            crate::logger::early_print("[PAGING] Created PD for high memory\n");
        } else if pdp_entry & PAGE_HUGE != 0 {
            crate::logger::early_print("[PAGING] PDP[3] is a 1GB huge page, cannot add APIC\n");
            return;
        } else {
            pd = (pdp_entry & 0x000F_FFFF_FFFF_F000) as *mut u64;
        }
        
        // Map Local APIC at 0xFEE00000 (PD index 503)
        // 0xFEE00000 >> 21 = 0x7F7, & 0x1FF = 503
        let apic_pd_idx = ((apic_addr >> 21) & 0x1FF) as usize;
        let apic_entry = apic_addr | PAGE_PRESENT | PAGE_WRITE | PAGE_HUGE | PAGE_NO_CACHE | PAGE_WRITE_THROUGH;
        core::ptr::write_volatile(pd.add(apic_pd_idx), apic_entry);
        
        // Map I/O APIC at 0xFEC00000 (PD index 502)
        let ioapic_pd_idx = ((ioapic_addr >> 21) & 0x1FF) as usize;
        let ioapic_entry = ioapic_addr | PAGE_PRESENT | PAGE_WRITE | PAGE_HUGE | PAGE_NO_CACHE | PAGE_WRITE_THROUGH;
        core::ptr::write_volatile(pd.add(ioapic_pd_idx), ioapic_entry);
        
        // Flush TLB
        core::arch::asm!("mov {tmp}, cr3", "mov cr3, {tmp}", tmp = out(reg) _, options(nostack));
        
        crate::logger::early_print("[PAGING] ✓ APIC regions mapped (0xFEC00000, 0xFEE00000)\n");
    }
}
