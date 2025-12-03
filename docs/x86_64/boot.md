# 🚀 Boot Sequence

## Étapes de Boot

```
┌─────────────────────────────────────────────────────────────┐
│  BIOS/UEFI → Bootloader → kernel_main()                     │
├─────────────────────────────────────────────────────────────┤
│  1. Early init (serial, VGA)                                │
│  2. GDT setup (segments)                                    │
│  3. IDT setup (interrupts)                                  │
│  4. Paging init (identity + higher half)                    │
│  5. Heap init                                               │
│  6. APIC init                                               │
│  7. Scheduler init                                          │
│  8. IPC init                                                │
│  9. VFS init                                                │
│  10. User space ready                                       │
└─────────────────────────────────────────────────────────────┘
```

## GDT (Global Descriptor Table)

```rust
pub struct Gdt {
    null: u64,           // Entrée nulle obligatoire
    kernel_code: u64,    // CS kernel (ring 0)
    kernel_data: u64,    // DS/SS kernel
    user_code: u64,      // CS user (ring 3)
    user_data: u64,      // DS/SS user
    tss: [u64; 2],       // TSS (16 bytes)
}
```

### Sélecteurs

| Sélecteur | Offset | Ring | Usage |
|-----------|--------|------|-------|
| NULL | 0x00 | - | Obligatoire |
| KERNEL_CODE | 0x08 | 0 | Code kernel |
| KERNEL_DATA | 0x10 | 0 | Data kernel |
| USER_CODE | 0x18 | 3 | Code user |
| USER_DATA | 0x20 | 3 | Data user |
| TSS | 0x28 | 0 | Task State |

## IDT (Interrupt Descriptor Table)

```rust
#[repr(C, packed)]
pub struct IdtEntry {
    offset_low: u16,      // Bits 0-15 du handler
    selector: u16,        // CS selector
    ist: u8,              // Interrupt Stack Table
    type_attr: u8,        // Type et attributs
    offset_mid: u16,      // Bits 16-31
    offset_high: u32,     // Bits 32-63
    reserved: u32,
}
```

### Vecteurs Configurés

| Vecteur | Nom | Handler |
|---------|-----|---------|
| 0 | Division Error | `division_error_handler` |
| 8 | Double Fault | `double_fault_handler` (IST1) |
| 13 | General Protection | `gp_fault_handler` |
| 14 | Page Fault | `page_fault_handler` |
| 32-47 | IRQs | `irq_handler_N` |
| 0x80 | Syscall (legacy) | `syscall_handler` |

## TSS (Task State Segment)

```rust
#[repr(C, packed)]
pub struct Tss {
    reserved0: u32,
    rsp0: u64,           // Stack pour ring 0
    rsp1: u64,           // Stack pour ring 1
    rsp2: u64,           // Stack pour ring 2
    reserved1: u64,
    ist1: u64,           // Interrupt Stack 1 (double fault)
    ist2: u64,           // Interrupt Stack 2
    // ...
    ist7: u64,
    reserved2: u64,
    reserved3: u16,
    iopb_offset: u16,    // I/O Permission Bitmap
}
```

## Initialisation

```rust
pub fn init() {
    // 1. Setup GDT
    gdt::init();
    
    // 2. Setup TSS
    tss::init();
    
    // 3. Setup IDT
    idt::init();
    
    // 4. Enable interrupts
    unsafe { asm!("sti"); }
}
```
