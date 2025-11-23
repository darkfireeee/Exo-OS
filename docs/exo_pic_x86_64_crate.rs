// kernel/src/arch/x86_64/interrupts/pic_x86_64.rs
//
// IMPLÉMENTATION PIC AVEC LA CRATE X86_64 (ALTERNATIVE)

use x86_64::instructions::port::{Port, PortWriteOnly, PortReadOnly};

/// Ports I/O du PIC
struct PicPorts {
    command: PortWriteOnly<u8>,
    data: Port<u8>,
}

impl PicPorts {
    const fn new(command_port: u16, data_port: u16) -> Self {
        PicPorts {
            command: PortWriteOnly::new(command_port),
            data: Port::new(data_port),
        }
    }
}

/// Structure du PIC
pub struct Pic {
    master: PicPorts,
    slave: PicPorts,
    master_offset: u8,
    slave_offset: u8,
}

impl Pic {
    /// Crée une nouvelle instance
    pub const fn new(master_offset: u8, slave_offset: u8) -> Self {
        Pic {
            master: PicPorts::new(0x20, 0x21),
            slave: PicPorts::new(0xA0, 0xA1),
            master_offset,
            slave_offset,
        }
    }

    /// Initialise le PIC avec remapping
    pub unsafe fn init(&mut self) {
        println!("[PIC] Starting initialization...");
        
        // Sauvegarder les masques
        let master_mask = self.master.data.read();
        let slave_mask = self.slave.data.read();
        
        println!("[PIC] Saved masks: Master={:#04x}, Slave={:#04x}", 
                 master_mask, slave_mask);

        // ICW1: Init en cascade mode
        self.master.command.write(0x11);
        io_wait();
        self.slave.command.write(0x11);
        io_wait();
        
        println!("[PIC] ICW1 sent (init cascade)");

        // ICW2: Vecteurs d'interruption
        self.master.data.write(self.master_offset);
        io_wait();
        self.slave.data.write(self.slave_offset);
        io_wait();
        
        println!("[PIC] ICW2 sent (offsets: master={}, slave={})", 
                 self.master_offset, self.slave_offset);

        // ICW3: Configuration cascade
        self.master.data.write(0x04);  // Slave sur IRQ2
        io_wait();
        self.slave.data.write(0x02);   // Cascade ID 2
        io_wait();
        
        println!("[PIC] ICW3 sent (cascade config)");

        // ICW4: Mode 8086
        self.master.data.write(0x01);
        io_wait();
        self.slave.data.write(0x01);
        io_wait();
        
        println!("[PIC] ICW4 sent (8086 mode)");

        // Masquer toutes les IRQs initialement
        self.master.data.write(0xFF);
        self.slave.data.write(0xFF);
        
        println!("[PIC] All IRQs masked");
    }

    /// Active une IRQ
    pub unsafe fn unmask(&mut self, irq: u8) {
        if irq < 8 {
            let mask = self.master.data.read();
            self.master.data.write(mask & !(1 << irq));
            println!("[PIC] Unmasked IRQ {}", irq);
        } else {
            let mask = self.slave.data.read();
            self.slave.data.write(mask & !(1 << (irq - 8)));
            println!("[PIC] Unmasked IRQ {}", irq);
        }
    }

    /// Envoie EOI
    pub unsafe fn send_eoi(&mut self, irq: u8) {
        if irq >= 8 {
            self.slave.command.write(0x20);
        }
        self.master.command.write(0x20);
    }

    /// Désactive les PICs
    pub unsafe fn disable(&mut self) {
        self.master.data.write(0xFF);
        self.slave.data.write(0xFF);
    }
}

/// Wait I/O
unsafe fn io_wait() {
    let mut port = Port::<u8>::new(0x80);
    port.write(0);
}

// Instance globale
static mut PIC: Pic = Pic::new(32, 40);

/// Init wrapper
pub fn init_pic() {
    unsafe {
        PIC.init();
        PIC.unmask(0);  // Timer
        PIC.unmask(1);  // Keyboard
    }
}

/// EOI wrapper
pub fn send_eoi(irq: u8) {
    unsafe {
        PIC.send_eoi(irq);
    }
}

/// Disable wrapper
pub fn disable_pic() {
    unsafe {
        PIC.disable();
    }
}

// Placeholder pour println
macro_rules! println {
    ($($arg:tt)*) => {
        // TODO: Utiliser votre implementation
    };
}
