//! I/O APIC support for IRQ routing

pub const IOAPIC_BASE: usize = 0xFEC00000;

// I/O APIC registers
const IOREGSEL: usize = 0x00;  // Register selector
const IOWIN: usize = 0x10;     // Register window

// I/O APIC register indices
const IOAPIC_ID: u8 = 0x00;
const IOAPIC_VER: u8 = 0x01;
const IOAPIC_ARB: u8 = 0x02;
const IOREDTBL_BASE: u8 = 0x10;

/// I/O APIC structure
pub struct IoApic {
    base_addr: usize,
}

impl IoApic {
    pub fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }
    
    fn write_reg(&mut self, reg: u8, value: u32) {
        unsafe {
            let regsel = self.base_addr as *mut u32;
            let win = (self.base_addr + IOWIN) as *mut u32;
            
            core::ptr::write_volatile(regsel, reg as u32);
            core::ptr::write_volatile(win, value);
        }
    }
    
    fn read_reg(&self, reg: u8) -> u32 {
        unsafe {
            let regsel = self.base_addr as *mut u32;
            let win = (self.base_addr + IOWIN) as *const u32;
            
            core::ptr::write_volatile(regsel, reg as u32);
            core::ptr::read_volatile(win)
        }
    }
    
    pub fn init(&mut self) {
        // Get number of redirection entries
        let ver = self.read_reg(IOAPIC_VER);
        let max_redirects = ((ver >> 16) & 0xFF) + 1;
        
        log::info!("I/O APIC initialized, {} redirection entries", max_redirects);
        
        // Mask all interrupts initially
        for i in 0..max_redirects {
            self.set_irq_mask(i as u8, true);
        }
    }
    
    pub fn set_irq_mask(&mut self, irq: u8, masked: bool) {
        let redtbl = IOREDTBL_BASE + (irq * 2);
        
        // Read current low dword
        let mut low = self.read_reg(redtbl);
        
        if masked {
            low |= 1 << 16; // Set mask bit
        } else {
            low &= !(1 << 16); // Clear mask bit
        }
        
        self.write_reg(redtbl, low);
    }
    
    pub fn route_irq(&mut self, irq: u8, vector: u8, apic_id: u8) {
        let redtbl = IOREDTBL_BASE + (irq * 2);
        
        // Low dword: vector + delivery mode (fixed) + logical dest mode
        let low = (vector as u32) | (0 << 8) | (0 << 11);
        
        // High dword: destination APIC ID
        let high = (apic_id as u32) << 24;
        
        self.write_reg(redtbl, low);
        self.write_reg(redtbl + 1, high);
        
        log::debug!("I/O APIC: routed IRQ {} to vector {} on APIC {}", 
            irq, vector, apic_id);
    }
    
    pub fn get_id(&self) -> u8 {
        ((self.read_reg(IOAPIC_ID) >> 24) & 0xF) as u8
    }
}

/// Global I/O APIC instance
static IO_APIC: spin::Once<spin::Mutex<IoApic>> = spin::Once::new();

/// Initialize I/O APIC
pub fn init() {
    let ioapic = IO_APIC.call_once(|| {
        let mut ioapic = IoApic::new(IOAPIC_BASE);
        ioapic.init();
        spin::Mutex::new(ioapic)
    });
    
    log::info!("I/O APIC initialized, ID = {}", ioapic.lock().get_id());
}

/// Set IRQ mask
pub fn set_irq_mask(irq: u8, masked: bool) {
    if let Some(ioapic) = IO_APIC.get() {
        ioapic.lock().set_irq_mask(irq, masked);
    }
}

/// Route IRQ to APIC
pub fn route_irq(irq: u8, vector: u8, apic_id: u8) {
    if let Some(ioapic) = IO_APIC.get() {
        ioapic.lock().route_irq(irq, vector, apic_id);
    }
}
