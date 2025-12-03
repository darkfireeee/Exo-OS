# 🗺️ Virtual Memory Management

## Page Tables x86_64

### Hiérarchie 4 Niveaux

```
┌─────────────────────────────────────────────────────────────┐
│                    Virtual Address (48 bits)                 │
├─────────────────────────────────────────────────────────────┤
│ Sign │  PML4  │  PDPT  │   PD   │   PT   │  Offset │        │
│ 16b  │  9 bits│  9 bits│  9 bits│  9 bits│  12 bits│        │
└─────────────────────────────────────────────────────────────┘
           │         │         │         │         │
           v         v         v         v         v
        ┌─────┐  ┌─────┐  ┌─────┐  ┌─────┐  ┌─────────┐
        │PML4 │→ │PDPT │→ │ PD  │→ │ PT  │→ │ Frame   │
        │Entry│  │Entry│  │Entry│  │Entry│  │ 4KB     │
        └─────┘  └─────┘  └─────┘  └─────┘  └─────────┘
```

### Page Table Entry

```rust
bitflags! {
    pub struct PageFlags: u64 {
        const PRESENT    = 1 << 0;   // Page présente
        const WRITABLE   = 1 << 1;   // Écriture autorisée
        const USER       = 1 << 2;   // Accessible en ring 3
        const PWT        = 1 << 3;   // Write-through
        const PCD        = 1 << 4;   // Cache disabled
        const ACCESSED   = 1 << 5;   // Page accédée
        const DIRTY      = 1 << 6;   // Page modifiée
        const HUGE       = 1 << 7;   // 2MB/1GB page
        const GLOBAL     = 1 << 8;   // Ne pas flush TLB
        const NO_EXECUTE = 1 << 63;  // Non exécutable (NX)
    }
}
```

## API

```rust
// Mapper une page
page_table.map(
    VirtualAddress::new(0x1000),
    PhysicalAddress::new(0x2000),
    PageFlags::PRESENT | PageFlags::WRITABLE
)?;

// Unmapper
page_table.unmap(VirtualAddress::new(0x1000))?;

// Traduire
let phys = page_table.translate(VirtualAddress::new(0x1000))?;

// Changer permissions
page_table.remap(virt, PageFlags::PRESENT)?; // Read-only
```

## TLB Management

```rust
// Flush une entrée
pub fn flush_tlb(addr: VirtualAddress) {
    unsafe {
        asm!("invlpg [{}]", in(reg) addr.as_u64());
    }
}

// Flush tout
pub fn flush_tlb_all() {
    unsafe {
        let cr3: u64;
        asm!("mov {}, cr3", out(reg) cr3);
        asm!("mov cr3, {}", in(reg) cr3);
    }
}

// Flush avec PCID (si disponible)
pub fn flush_tlb_pcid(pcid: u16, addr: VirtualAddress) {
    let descriptor = (pcid as u64) | (addr.as_u64() & !0xFFF);
    unsafe {
        asm!("invpcid {}, [{}]", in(reg) 0u64, in(reg) &descriptor);
    }
}
```

## Memory Map Kernel

```
0xFFFF_8000_0000_0000 ─┬─ Higher Half Start
                       │
0xFFFF_8000_0000_0000 ─┼─ Direct Physical Map (identity)
                       │  Tout l'espace physique mappé
                       │
0xFFFF_C000_0000_0000 ─┼─ Kernel Code/Data
                       │
0xFFFF_D000_0000_0000 ─┼─ Kernel Heap
                       │
0xFFFF_E000_0000_0000 ─┼─ Kernel Stacks
                       │
0xFFFF_F000_0000_0000 ─┼─ Device MMIO
                       │
0xFFFF_FFFF_FFFF_FFFF ─┴─ End
```
