# 🏗️ ARCHITECTURE - Exo-OS v0.5.0 "Linux Crusher"

**Version**: 0.5.0 "Linux Crusher"  
**Date**: 4 décembre 2025  
**Architecture Cible**: x86_64

---

## 📋 Table des Matières

1. [Vue d'ensemble](#vue-densemble)
2. [Architecture Globale](#architecture-globale)
3. [Sous-systèmes Majeurs](#sous-systèmes-majeurs)
4. [Boot Sequence](#boot-sequence)
5. [Scheduler 3-Queue EMA](#scheduler-3-queue-ema)
6. [Mémoire et MMU](#mémoire-et-mmu)
7. [IPC et Pipes](#ipc-et-pipes)
8. [Performances](#performances)

---

## 🎯 Vue d'ensemble

Exo-OS v0.5.0 est un microkernel moderne orienté **ultra-performance** visant à surpasser Linux :

| Métrique | Linux | Exo-OS Target | Gain |
|----------|-------|---------------|------|
| IPC Latency | 1,250 cycles | 347 cycles | 3.6x |
| Context Switch | 2,150 cycles | 304 cycles | 7x |
| Syscall | 150 cycles | 45 cycles | 3.3x |

### Caractéristiques v0.5.0

- ✅ **Boot multiboot2** - ASM → C → Rust linkage complet
- ✅ **Scheduler 3-Queue EMA** avec préemption timer (10ms)
- ✅ **Heap allocator** 10MB stable (linked-list)
- ✅ **tmpfs** complet avec hashbrown O(1)
- ✅ **pipe() syscall** pour IPC basique
- ✅ **Clavier PS/2** avec IRQ1 handler
- ✅ **Module MMU** avec TLB invalidation
- ✅ **Benchmark infrastructure** (rdtsc/rdtscp)

---

## 🏛️ Architecture Globale

```
┌─────────────────────────────────────────────────────────────────┐
│                         USERLAND (Futur v0.6.0+)               │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐   │
│  │  Shell  │  │   AI    │  │   Net   │  │  FS Service     │   │
│  │         │  │  Core   │  │ Service │  │                 │   │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────────┬────────┘   │
└───────┼───────────┼──────────────┼─────────────────┼────────────┘
        │           │              │                 │
        │  IPC (Named Channels / Pipes)             │
        │           │              │                 │
┌───────▼───────────▼──────────────▼─────────────────▼────────────┐
│                    KERNEL SPACE (v0.5.0)                        │
│  ┌────────────────────────────────────────────────────────────┐│
│  │               Syscall Dispatcher (256 entries)             ││
│  └───┬─────────┬──────────┬──────────┬──────────┬─────────────┘│
│      │         │          │          │          │              │
│  ┌───▼──┐  ┌──▼───┐  ┌───▼───┐  ┌──▼───┐  ┌───▼────┐         │
│  │Memory│  │ Time │  │  I/O  │  │ IPC  │  │Drivers │         │
│  │      │  │      │  │       │  │      │  │        │         │
│  │·mmap │  │·PIT  │  │·tmpfs │  │·pipe │  │·PS/2   │         │
│  │·heap │  │·TSC  │  │·VFS   │  │·chan │  │·Serial │         │
│  │·MMU  │  │·RTC  │  │·FD    │  │·shm  │  │·VGA    │         │
│  └───┬──┘  └──┬───┘  └───┬───┘  └──┬───┘  └───┬────┘         │
│      │        │          │          │          │              │
│  ┌───▼────────▼──────────▼──────────▼──────────▼───────┐      │
│  │              Core Infrastructure                     │      │
│  │  • Scheduler 3-Queue EMA (Hot/Normal/Cold)          │      │
│  │  • Frame Allocator (bitmap 64-bit chunks)           │      │
│  │  • Interrupts (PIC8259 + IDT64)                     │      │
│  │  • Benchmark Module (rdtsc cycles)                  │      │
│  └──────────────────────────────────────────────────────┘      │
│                                                                 │
│  ┌──────────────────────────────────────────────────────┐      │
│  │                   Boot Layer                         │      │
│  │  boot.asm (32→64) → boot.c (FFI) → rust_main()      │      │
│  └──────────────────────────────────────────────────────┘      │
└─────────────────────────────────────────────────────────────────┘
```

---

## 🚀 Boot Sequence

### Flux de démarrage

```
GRUB (multiboot2)
    │
    ▼
boot.asm (kernel/src/arch/x86_64/boot/boot.asm)
    │ ├── Setup GDT64 (code/data segments)
    │ ├── Enable Long Mode (CR0.PG, CR4.PAE, EFER.LME)
    │ ├── Setup Identity Paging (8GB P2 huge pages)
    │ └── Jump to 64-bit
    │
    ▼
boot.c (kernel/src/arch/x86_64/boot/boot.c)
    │ ├── c_boot_init() - FFI bridge
    │ ├── Preserve multiboot2 info
    │ └── Call rust_main(magic, info_ptr)
    │
    ▼
rust_main() (kernel/src/lib.rs)
    │ ├── Serial/VGA init
    │ ├── GDT + IDT install
    │ ├── PIC8259 + PIT 100Hz
    │ ├── Frame allocator (bitmap)
    │ ├── Heap allocator (10MB)
    │ ├── Scheduler init
    │ ├── Keyboard IRQ1
    │ └── Demo threads
    │
    ▼
Scheduler Loop (preemptive)
```

### Memory Layout

```
0x0000_0000 - 0x0010_0000 : BIOS, VGA (1MB)
0x0010_0000 - 0x0050_0000 : Kernel Code (4MB)
0x0050_0000 - 0x0050_4000 : Bitmap (16KB, 512MB tracking)
0x0080_0000 - 0x00A8_0000 : Heap (10MB linked-list)
0x0100_0000 - 0x2_0000_0000 : Available frames (8GB max)
```

---

## ⚡ Scheduler 3-Queue EMA

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                 Scheduler 3-Queue EMA                   │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐    │
│  │  HOT Queue  │  │NORMAL Queue │  │ COLD Queue  │    │
│  │  (CPU-bound)│  │  (Mixed)    │  │ (I/O-bound) │    │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘    │
│         │                │                │            │
│         └────────────────┼────────────────┘            │
│                          │                             │
│  ┌───────────────────────▼─────────────────────────┐  │
│  │         EMA (Exponential Moving Average)        │  │
│  │         α = 0.5 for adaptive scheduling         │  │
│  └─────────────────────────────────────────────────┘  │
│                                                         │
│  Timer IRQ0 (PIT 100Hz) → Preemption every 10 ticks   │
│  Quantum: 10ms (100 ticks/sec × 10 ticks = 10ms)      │
└─────────────────────────────────────────────────────────┘
```

### Windowed Context Switch

```rust
// Minimal register save (only caller-saved)
// rbx, r12-r15, rsp, rip preserved by convention
// Only save: rax, rcx, rdx, rsi, rdi, r8-r11

Context {
    rsp: u64,      // Stack pointer
    rip: u64,      // Instruction pointer
    rflags: u64,   // Flags
    // Windowed: skip rbx, rbp, r12-r15 (callee-saved)
}
```

---

## 🧠 Mémoire et MMU

### Frame Allocator

```rust
// Bitmap allocator: 64-bit chunks for fast scanning
struct FrameAllocator {
    bitmap: &'static mut [u64],  // 1 bit = 1 frame (4KB)
    total_frames: usize,
    free_frames: AtomicUsize,
}

// O(64) scan per allocation (TZCNT instruction)
fn allocate_frame() -> Option<PhysFrame> {
    for chunk in bitmap {
        if *chunk != u64::MAX {
            let bit = chunk.trailing_ones();
            *chunk |= 1 << bit;
            return Some(frame_at(bit));
        }
    }
    None
}
```

### MMU Functions (Real Implementation)

```rust
// kernel/src/arch/mod.rs

pub fn map_temporary(phys: u64, size: usize) -> *mut u8 {
    // Identity mapping for kernel space
    phys as *mut u8
}

pub fn invalidate_tlb(addr: u64) {
    unsafe { asm!("invlpg [{}]", in(reg) addr, options(nostack)); }
}

pub fn invalidate_tlb_all() {
    unsafe {
        let cr3: u64;
        asm!("mov {}, cr3", out(reg) cr3);
        asm!("mov cr3, {}", in(reg) cr3);  // CR3 reload flushes TLB
    }
}

pub fn get_page_table_root() -> u64 {
    unsafe {
        let cr3: u64;
        asm!("mov {}, cr3", out(reg) cr3);
        cr3
    }
}
```

---

## 📡 IPC et Pipes

### Named Channels

```rust
// kernel/src/ipc/named.rs

pub struct NamedChannel {
    name: String,
    buffer: VecDeque<Message>,
    capacity: usize,
    readers: AtomicUsize,
    writers: AtomicUsize,
}

// pipe() syscall
pub fn pipe() -> Result<(FileDescriptor, FileDescriptor), IpcError> {
    let channel = NamedChannel::anonymous(PIPE_BUFFER_SIZE);
    let read_fd = channel.create_reader();
    let write_fd = channel.create_writer();
    Ok((read_fd, write_fd))
}
```

### Syscall Registration

```rust
// SYS_PIPE = 22
register_syscall(SYS_PIPE, |args| {
    let pipefd = args[0] as *mut [i32; 2];
    match ipc::sys_pipe() {
        Ok((read_fd, write_fd)) => {
            unsafe {
                (*pipefd)[0] = read_fd;
                (*pipefd)[1] = write_fd;
            }
            0
        }
        Err(_) => -1,
    }
});
```

---

## 📊 Performances

### Benchmark Module

```rust
// kernel/src/bench/mod.rs

#[inline(always)]
pub fn rdtsc() -> u64 {
    unsafe {
        let lo: u32;
        let hi: u32;
        asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
        ((hi as u64) << 32) | (lo as u64)
    }
}

#[macro_export]
macro_rules! measure {
    ($name:expr, $code:block) => {{
        let start = $crate::bench::rdtsc();
        let result = $code;
        let end = $crate::bench::rdtsc();
        $crate::bench::BenchResult {
            name: $name,
            cycles: end - start,
            result,
        }
    }};
}
```

### Targets v1.0.0

| Opération | Target | Status |
|-----------|--------|--------|
| IPC Send/Recv | < 350 cycles | 🔄 En cours |
| Context Switch | < 500 cycles | ✅ Infrastructure prête |
| Syscall Entry | < 50 cycles | 🔄 En cours |
| Page Fault | < 1000 cycles | 📅 Planifié |

---

## 📁 Structure du Code

```
kernel/src/
├── lib.rs              # Entry point (rust_main)
├── splash.rs           # Boot splash v0.5.0
├── logger.rs           # Serial logging
├── bench/              # Benchmark infrastructure
│   └── mod.rs          # rdtsc, measure! macro
├── arch/
│   ├── mod.rs          # MMU functions (real impl)
│   └── x86_64/
│       ├── boot/       # boot.asm + boot.c
│       ├── gdt.rs      # GDT64
│       ├── idt.rs      # IDT + handlers
│       ├── pic.rs      # PIC8259
│       └── syscall.rs  # Syscall dispatch
├── memory/
│   ├── frame.rs        # Frame allocator (bitmap)
│   ├── heap.rs         # Heap (10MB linked-list)
│   └── mmap.rs         # mmap structures
├── scheduler/
│   └── mod.rs          # 3-Queue EMA scheduler
├── ipc/
│   └── named.rs        # Named channels + pipe()
├── fs/
│   └── vfs/
│       └── tmpfs.rs    # RAM filesystem
└── drivers/
    └── input/
        └── keyboard.rs # PS/2 driver
```

---

## 🔮 Roadmap

### v0.5.0 (Actuel) - Phase 0 Complete ✅
- Boot ISO fonctionnel
- Scheduler avec préemption
- IPC basique (pipe)
- Keyboard driver

### v0.6.0 (Prochain) - Phase 1
- fork/exec/wait syscalls
- Shell interactif
- VFS mount/unmount
- ELF loader

### v1.0.0 (Vision) - Linux Crusher
- Surpasser Linux en performance
- Network stack TCP/IP
- ext2/FAT32 filesystem
- Multi-core SMP

---

## 📄 License

GPL-2.0 - Permet l'utilisation de code Linux (drivers, etc.)

---

*Exo-OS v0.5.0 "Linux Crusher" - Making the impossible possible* 🚀
