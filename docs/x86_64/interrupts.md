# ⚡ Interrupts & APIC

## Architecture des Interruptions

```
┌─────────────────────────────────────────────────────────────┐
│                    Interrupt Flow                            │
├─────────────────────────────────────────────────────────────┤
│  Hardware IRQ → I/O APIC → Local APIC → CPU → IDT → Handler │
│  Software INT → CPU → IDT → Handler                         │
│  Exception    → CPU → IDT → Handler                         │
└─────────────────────────────────────────────────────────────┘
```

## Local APIC

### Registres

```rust
pub const LAPIC_ID: u32 = 0x020;          // ID du CPU
pub const LAPIC_VERSION: u32 = 0x030;     // Version
pub const LAPIC_TPR: u32 = 0x080;         // Task Priority
pub const LAPIC_EOI: u32 = 0x0B0;         // End of Interrupt
pub const LAPIC_SVR: u32 = 0x0F0;         // Spurious Vector
pub const LAPIC_ICR_LOW: u32 = 0x300;     // Inter-CPU Interrupt (low)
pub const LAPIC_ICR_HIGH: u32 = 0x310;    // Inter-CPU Interrupt (high)
pub const LAPIC_TIMER: u32 = 0x320;       // Timer LVT
pub const LAPIC_TIMER_INIT: u32 = 0x380;  // Timer Initial Count
pub const LAPIC_TIMER_CURRENT: u32 = 0x390; // Timer Current Count
pub const LAPIC_TIMER_DIV: u32 = 0x3E0;   // Timer Divider
```

### Initialisation

```rust
pub fn init_local_apic() {
    // Enable APIC via SVR
    let svr = read_lapic(LAPIC_SVR);
    write_lapic(LAPIC_SVR, svr | 0x100 | SPURIOUS_VECTOR);
    
    // Setup timer
    write_lapic(LAPIC_TIMER_DIV, 0x03);  // Divide by 16
    write_lapic(LAPIC_TIMER, TIMER_VECTOR | 0x20000); // Periodic
    write_lapic(LAPIC_TIMER_INIT, TIMER_COUNT);
}
```

## I/O APIC

### Redirection Table

```rust
pub struct IoApicEntry {
    pub vector: u8,           // IDT vector (32-255)
    pub delivery_mode: u8,    // 0=Fixed, 1=LowPri, 2=SMI, etc.
    pub dest_mode: bool,      // 0=Physical, 1=Logical
    pub polarity: bool,       // 0=Active High, 1=Active Low
    pub trigger: bool,        // 0=Edge, 1=Level
    pub mask: bool,           // 0=Enabled, 1=Masked
    pub destination: u8,      // APIC ID destination
}
```

### Configuration IRQ

```rust
pub fn configure_irq(irq: u8, vector: u8, cpu: u8) {
    let entry = IoApicEntry {
        vector,
        delivery_mode: 0,     // Fixed
        dest_mode: false,     // Physical
        polarity: false,      // Active High
        trigger: false,       // Edge
        mask: false,          // Enabled
        destination: cpu,
    };
    
    write_ioapic_entry(irq, entry);
}
```

## Handlers d'Interruption

### Structure

```rust
#[repr(C)]
pub struct InterruptStackFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}
```

### Handler Exemple

```rust
extern "x86-interrupt" fn timer_handler(stack_frame: InterruptStackFrame) {
    // Acknowledge interrupt
    unsafe { write_lapic(LAPIC_EOI, 0); }
    
    // Update tick count
    TICK_COUNT.fetch_add(1, Ordering::Relaxed);
    
    // Check preemption
    if scheduler::should_preempt() {
        scheduler::yield_now();
    }
}
```

## Inter-Processor Interrupts (IPI)

```rust
pub fn send_ipi(target_cpu: u8, vector: u8) {
    // Write destination
    write_lapic(LAPIC_ICR_HIGH, (target_cpu as u32) << 24);
    
    // Send IPI
    write_lapic(LAPIC_ICR_LOW, vector as u32);
    
    // Wait for delivery
    while read_lapic(LAPIC_ICR_LOW) & (1 << 12) != 0 {
        core::hint::spin_loop();
    }
}

// IPI types
pub const IPI_RESCHEDULE: u8 = 0xFE;   // Force reschedule
pub const IPI_TLB_SHOOTDOWN: u8 = 0xFD; // Flush TLB
pub const IPI_HALT: u8 = 0xFC;          // Halt CPU
```

## Timer APIC

```rust
pub fn calibrate_apic_timer() -> u64 {
    // Use PIT to calibrate
    let pit_freq = 1193182; // Hz
    let ms_count = 10;
    
    // Start APIC timer
    write_lapic(LAPIC_TIMER_INIT, 0xFFFFFFFF);
    
    // Wait using PIT
    pit_wait_ms(ms_count);
    
    // Read elapsed
    let elapsed = 0xFFFFFFFF - read_lapic(LAPIC_TIMER_CURRENT);
    
    // Calculate ticks per ms
    elapsed / ms_count
}
```
