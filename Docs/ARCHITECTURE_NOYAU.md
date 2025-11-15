# Architecture Complète du Noyau Exo-OS

**Version** : 0.1.0  
**Architecture** : x86_64  
**Date** : 10 novembre 2025  
**Langage** : Rust (nightly) + ASM + C

---

## Table des Matières

1. [Vue d'Ensemble](#vue-densemble)
2. [Structure des Fichiers](#structure-des-fichiers)
3. [Séquence de Boot](#séquence-de-boot)
4. [Modules du Noyau](#modules-du-noyau)
5. [Gestion Mémoire](#gestion-mémoire)
6. [Architecture x86_64](#architecture-x86_64)
7. [Ordonnanceur](#ordonnanceur)
8. [Système IPC](#système-ipc)
9. [Appels Système](#appels-système)
10. [Pilotes](#pilotes)
11. [Performances](#performances)
12. [Configuration Build](#configuration-build)

---

## Vue d'Ensemble

### Caractéristiques Principales

- **Type** : Noyau bare-metal x86_64 en mode protégé 64-bit
- **Bootloader** : GRUB 2 avec Multiboot2
- **No-std** : Pas de bibliothèque standard Rust
- **Allocateur** : linked_list_allocator pour le heap kernel
- **Interruptions** : IDT avec handlers x86_interrupt
- **Multitâche** : Ordonnanceur coopératif avec context switching en ASM
- **IPC** : Système de canaux inter-processus
- **Syscalls** : Interface d'appels système simplifiée

### Dépendances Principales

```toml
x86_64 = "0.14.13"              # Abstractions x86_64
spin = "0.9.8"                   # Spinlocks pour synchronisation
linked_list_allocator = "0.10.5" # Allocateur heap
multiboot2 = "0.22.2"            # Parsing info bootloader
uart_16550 = "0.2.19"            # Port série (debug)
pic8259 = "0.10.4"               # Contrôleur interruptions
lazy_static = "1.5.0"            # Statics avec initialisation lazy
```

---

## Structure des Fichiers

```
Exo-OS/
├── Cargo.toml                    # Workspace Rust
├── linker.ld                     # Script linker (sections mémoire)
├── x86_64-unknown-none.json      # Target custom bare-metal
├── build.rs                      # Build script (compile ASM)
│
├── kernel/
│   ├── Cargo.toml                # Config kernel
│   ├── build.rs                  # Compile context_switch.S + C
│   │
│   └── src/
│       ├── lib.rs                # Point d'entrée bibliothèque
│       ├── main.rs               # Point d'entrée binaire
│       │
│       ├── arch/                 # Architecture-specific code
│       │   ├── mod.rs
│       │   └── x86_64/
│       │       ├── mod.rs        # Interface x86_64
│       │       ├── boot.asm      # Bootstrap ASM (32-bit → 64-bit)
│       │       ├── boot.c        # Bootstrap C (appel Rust)
│       │       ├── gdt.rs        # Global Descriptor Table
│       │       ├── idt.rs        # Interrupt Descriptor Table
│       │       └── interrupts.rs # Handlers interruptions
│       │
│       ├── memory/               # Gestion mémoire
│       │   ├── mod.rs            # Interface publique
│       │   ├── frame_allocator.rs # Allocateur frames physiques
│       │   ├── page_table.rs     # Tables de pages virtuelles
│       │   └── heap_allocator.rs # Allocateur heap kernel
│       │
│       ├── scheduler/            # Ordonnancement
│       │   ├── mod.rs            # Interface ordonnanceur
│       │   ├── scheduler.rs      # Logique scheduling
│       │   ├── thread.rs         # Structure Thread
│       │   └── context_switch.S  # Context switch ASM
│       │
│       ├── ipc/                  # Inter-Process Communication
│       │   ├── mod.rs
│       │   ├── channel.rs        # Canaux de communication
│       │   └── message.rs        # Messages IPC
│       │
│       ├── syscall/              # Appels système
│       │   ├── mod.rs            # Interface syscall
│       │   └── dispatch.rs       # Dispatch syscalls
│       │
│       ├── drivers/              # Pilotes
│       │   ├── mod.rs            # Gestionnaire pilotes
│       │   ├── serial.rs         # Port série
│       │   └── block/
│       │       └── mod.rs        # Pilotes bloc (disques)
│       │
│       ├── libutils/             # Utilitaires
│       │   ├── mod.rs
│       │   ├── display.rs        # Affichage VGA
│       │   ├── macros/
│       │   │   ├── mod.rs
│       │   │   ├── lazy_static.rs
│       │   │   └── println.rs
│       │   ├── sync/
│       │   │   ├── mod.rs
│       │   │   └── mutex.rs
│       │   └── arch/
│       │       └── x86_64/
│       │           ├── mod.rs
│       │           └── interrupts.rs
│       │
│       ├── perf_counters.rs      # Mesures performance (RDTSC)
│       ├── boot_sequence.rs      # Séquence boot optimisée
│       └── c_compat/             # Code C (serial, PCI)
│           ├── mod.rs
│           ├── serial.c
│           └── pci.c
│
├── scripts/
│   ├── build-iso.sh              # Création ISO bootable
│   └── run-qemu.sh               # Lancement QEMU
│
└── Docs/
    ├── readme_kernel.txt
    ├── readme_memory_and_scheduler.md
    ├── readme_syscall_et_drivers.md
    ├── readme_x86_64_et_c_compact.md
    ├── phase1_optimization_plan.md
    ├── PHASE0_CORRECTIONS.md
    └── ARCHITECTURE_NOYAU.md     # Ce fichier
```

---

## Séquence de Boot

### 1. Bootstrap (boot.asm + boot.c)

```
GRUB/Multiboot2
    ↓
boot.asm (32-bit)
    ├─ Valide magic Multiboot2
    ├─ Configure stack
    ├─ Active mode long (64-bit)
    └─ Saute vers boot.c
         ↓
boot.c (64-bit)
    └─ Appelle rust_kernel_main
         ↓
kernel_main (lib.rs)
```

**Détails boot.asm** :
- Définit section `.multiboot_header` avec magic `0xe85250d6`
- Configure page tables initiales (identity mapping)
- Active PAE, Long Mode, paging
- Passe contrôle à `boot_start64` (64-bit)

**Détails boot.c** :
- Reçoit pointeur Multiboot2 info
- Appelle `extern void rust_kernel_main(void* multiboot_info)`

### 2. Initialisation Kernel (kernel_main)

**Ordre d'exécution actuel** :

```rust
pub fn kernel_main(multiboot_info_addr: usize) -> ! {
    // 1. Validation Multiboot2
    let boot_info = unsafe { 
        multiboot2::load(multiboot_info_addr) 
    };
    
    // 2. Initialisation Port Série (debug)
    drivers::serial::init();
    println!("[BOOT] Multiboot2 magic validé");
    
    // 3. Initialisation Architecture
    arch::x86_64::init(4); // 4 CPUs
    //   ├─ gdt::init()      → GDT + TSS
    //   ├─ idt::init()      → IDT + handlers
    //   └─ interrupts::init() → PIC, timer
    
    // 4. Initialisation Mémoire
    memory::init(boot_info);
    //   ├─ frame_allocator  → bitmap allocator
    //   ├─ page_table       → offset page table
    //   └─ heap_allocator   → linked list heap (16 MB)
    
    // 5. Séquence Boot Optimisée
    boot_sequence::run_boot_sequence();
    //   ├─ Phase CRITICAL:  scheduler, IPC
    //   ├─ Phase NORMAL:    syscall, drivers
    //   └─ Phase DEFERRED:  perf_counters
    
    // 6. Affichage VGA
    libutils::display::write_banner();
    
    // 7. Rapport Performance
    perf_counters::print_summary_report();
    
    // 8. Boucle Principale
    loop {
        x86_64::instructions::hlt();
    }
}
```

### 3. Phases d'Initialisation (boot_sequence.rs)

```rust
enum BootPhase {
    Critical,  // Bloquant, critique pour boot
    Normal,    // Important mais peut être différé
    Deferred,  // Non-critique, lazy init possible
}

struct BootTask {
    name: &'static str,
    phase: BootPhase,
    init_fn: fn(),
}
```

**Tâches par phase** :

- **CRITICAL** :
  - `scheduler::init(4)` → Initialise ordonnanceur 4 CPUs
  - `ipc::init()` → Initialise système IPC

- **NORMAL** :
  - `syscall::init()` → Initialise table syscalls
  - `drivers::init()` → Charge pilotes

- **DEFERRED** :
  - `perf_counters::init()` → Active mesures perf
  - (Futurs drivers lazy-loaded)

---

## Modules du Noyau

### arch/x86_64

**Rôle** : Abstraction matérielle x86_64

**Fichiers clés** :
- `gdt.rs` : Global Descriptor Table (segments, TSS)
- `idt.rs` : Interrupt Descriptor Table (256 entrées)
- `interrupts.rs` : Handlers (timer, clavier, exceptions)

**GDT (Global Descriptor Table)** :
```rust
struct Gdt {
    code_selector: SegmentSelector,  // CS (Code Segment)
    data_selector: SegmentSelector,  // DS (Data Segment)
    tss_selector: SegmentSelector,   // TSS (Task State Segment)
}
```

**IDT (Interrupt Descriptor Table)** :
- 256 entrées (handlers)
- Exceptions CPU : 0-31 (Divide Error, Page Fault, etc.)
- IRQ matériels : 32-47 (Timer, Clavier, etc.)
- Syscalls : 48-255 (réservés)

**TSS (Task State Segment)** :
- IST (Interrupt Stack Table) : 7 piles dédiées
- IST[0] : Double Fault handler (stack overflow protection)

### memory

**Rôle** : Gestion mémoire physique et virtuelle

#### frame_allocator.rs

**Bitmap Frame Allocator** :
```rust
struct BitmapFrameAllocator {
    bitmap: &'static mut [u8],  // 1 bit = 1 frame (4 KiB)
    base_addr: usize,            // Adresse physique début
    num_frames: usize,           // Nombre total frames
    next_free: usize,            // Optimisation recherche
}
```

**Fonctions** :
- `allocate_frame()` → Trouve et marque frame libre
- `deallocate_frame(frame)` → Libère frame
- Fallback : Pool statique si Multiboot2 indisponible

#### page_table.rs

**Page Table Manager** :
```rust
pub struct PageTableManager {
    p4_table: &'static mut PageTable,  // PML4 (niveau 4)
    phys_offset: VirtAddr,              // Offset identity mapping
}
```

**Lazy Static** :
```rust
lazy_static! {
    pub static ref MAPPER: Mutex<OffsetPageTable<'static>> = {
        // Obtient CR3 (adresse PML4)
        let p4_addr = Cr3::read().0.start_address();
        let p4_table = unsafe { &mut *(p4_addr.as_u64() as *mut PageTable) };
        
        // Identity mapping (phys = virt)
        let phys_offset = VirtAddr::new(0);
        Mutex::new(OffsetPageTable::new(p4_table, phys_offset))
    };
}
```

**Mapping** :
- Identity mapped : 0x00000000 → 0x00000000 (premiers MB)
- Kernel : 0x00100000 → 0x00100000 (1 MiB+)
- VGA Buffer : 0xb8000 → 0xb8000 (text mode)
- Heap : Allocation dynamique après kernel

#### heap_allocator.rs

**Heap Kernel** :
```rust
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    const HEAP_START: usize = 0x4444_4444_0000; // Adresse virtuelle
    const HEAP_SIZE: usize = 16 * 1024 * 1024;  // 16 MB
    
    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }
}
```

**Utilisé par** :
- `alloc::vec::Vec<T>`
- `alloc::collections::BTreeMap<K, V>`
- `alloc::string::String`
- `alloc::boxed::Box<T>`

### scheduler

**Rôle** : Ordonnancement coopératif multi-threading

#### scheduler.rs

**Structure Ordonnanceur** :
```rust
pub struct Scheduler {
    threads: BTreeMap<ThreadId, Thread>,  // Tous les threads
    ready_queue: VecDeque<ThreadId>,      // File d'attente prêts
    current: Option<ThreadId>,            // Thread actuel
    next_tid: AtomicU64,                  // Prochain ID
    num_cpus: usize,                      // Nombre CPUs
}
```

**États Thread** :
```rust
pub enum ThreadState {
    Ready,      // Prêt à s'exécuter
    Running,    // En cours d'exécution
    Blocked,    // Bloqué (I/O, mutex, etc.)
    Terminated, // Terminé
}
```

**Algorithme** :
- Round-robin coopératif
- Pas de préemption (yield volontaire)
- Priorités futures (TODO)

#### thread.rs

**Structure Thread** :
```rust
pub struct Thread {
    id: ThreadId,
    state: ThreadState,
    context: ThreadContext,  // Registres sauvegardés
    stack: Option<Box<[u8]>>, // Stack propre
    name: Option<String>,
}

pub struct ThreadContext {
    // Registres généraux
    rax: u64, rbx: u64, rcx: u64, rdx: u64,
    rsi: u64, rdi: u64, rbp: u64, rsp: u64,
    r8: u64, r9: u64, r10: u64, r11: u64,
    r12: u64, r13: u64, r14: u64, r15: u64,
    
    // Registres contrôle
    rip: u64,    // Instruction pointer
    rflags: u64, // Flags
}
```

#### context_switch.S

**Context Switch ASM** :
```nasm
.global context_switch
context_switch:
    # Sauvegarde ancien contexte (RDI = *old_context)
    mov [rdi + 0x00], rax
    mov [rdi + 0x08], rbx
    # ... (tous les registres)
    
    # Restaure nouveau contexte (RSI = *new_context)
    mov rax, [rsi + 0x00]
    mov rbx, [rsi + 0x08]
    # ... (tous les registres)
    
    ret
```

**Stack Layout** :
```
High Address
+------------------+
| Thread Args      | ← Données thread
+------------------+
| Return Address   | ← Adresse retour
+------------------+
| RBP sauvegardé   |
+------------------+
| Registres        | ← Context
+------------------+
| Stack Guard      | ← Protection overflow
+------------------+
Low Address (RSP)
```

### ipc

**Rôle** : Communication inter-processus

#### channel.rs

**Structure Canal** :
```rust
pub struct Channel {
    name: String,
    messages: Mutex<VecDeque<Message>>,
    capacity: usize,          // Max messages
    blocked_senders: Mutex<VecDeque<ThreadId>>,
    blocked_receivers: Mutex<VecDeque<ThreadId>>,
}
```

**Canaux par défaut** :
- `system` : Messages système
- `log` : Logs kernel
- `user_input` : Entrées utilisateur
- `network` : Réseau (futur)

#### message.rs

**Structure Message** :
```rust
pub struct Message {
    data: MessageData,
    sender: Option<ThreadId>,
    priority: u8,
    timestamp: u64, // RDTSC
}

pub enum MessageData {
    FastMessage([u8; 64]),        // Petit message (stack)
    LargeMessage(Vec<u8>),         // Grand message (heap)
    SharedMemory { ptr: usize, size: usize },
}
```

**Optimisation** :
- Messages ≤64 bytes : stack (pas d'allocation)
- Messages >64 bytes : heap (Vec)
- Mémoire partagée : juste pointeurs

### syscall

**Rôle** : Interface appels système (userspace → kernel)

#### dispatch.rs

**Table Syscalls** :
```rust
pub enum Syscall {
    Exit = 0,
    Read = 1,
    Write = 2,
    Open = 3,
    Close = 4,
    Fork = 5,
    Exec = 6,
    Wait = 7,
    GetPid = 8,
    Sleep = 9,
    Mmap = 10,
    Munmap = 11,
    Clone = 12,
    // ... jusqu'à 256
}
```

**Arguments** :
```rust
pub struct SyscallArgs {
    rax: u64, // Numéro syscall
    rdi: u64, // Arg 1
    rsi: u64, // Arg 2
    rdx: u64, // Arg 3
    r10: u64, // Arg 4
    r8: u64,  // Arg 5
    r9: u64,  // Arg 6
}
```

**Dispatch** :
```rust
pub fn dispatch_syscall(args: &SyscallArgs) -> u64 {
    match args.rax {
        0 => sys_exit(args),
        1 => sys_read(args),
        2 => sys_write(args),
        // ...
        _ => u64::MAX, // ENOSYS
    }
}
```

**Convention** :
- Entrée : `syscall` instruction (x86_64)
- Registres : RAX (num), RDI-R9 (args)
- Retour : RAX (résultat ou errno)

### drivers

**Rôle** : Abstraction pilotes périphériques

#### mod.rs

**Driver Trait** :
```rust
pub trait Driver: Send + Sync {
    fn driver_type(&self) -> DriverType;
    fn name(&self) -> &str;
    fn init(&mut self) -> Result<(), DriverError>;
    fn shutdown(&mut self) -> Result<(), DriverError>;
    fn is_ready(&self) -> bool;
}

pub enum DriverType {
    Block,    // Disques, USB storage
    Char,     // Série, clavier
    Network,  // Ethernet, WiFi
    USB,      // Contrôleurs USB
    PCI,      // Périphériques PCI
    Unknown,
}
```

**Gestionnaire** :
```rust
pub struct DriverManager {
    drivers: BTreeMap<u32, Arc<Mutex<dyn Driver>>>,
    next_id: u32,
}

lazy_static! {
    pub static ref DRIVER_MANAGER: Mutex<DriverManager> = 
        Mutex::new(DriverManager::new());
}
```

#### serial.rs

**Port Série (UART 16550)** :
```rust
pub struct SerialPort {
    port: Port<u8>,  // Port I/O
    base_port: u16,  // 0x3F8 (COM1)
}

pub fn init() {
    let mut port = SerialPort::new(0x3F8); // COM1
    port.init();
    unsafe { SERIAL_PORT = Some(port); }
}

pub fn write_str(s: &str) {
    for byte in s.bytes() {
        write_byte(byte);
    }
}
```

**Utilisé par** :
- `println!` macro (debug output)
- Logs kernel
- Communication QEMU

#### block/mod.rs

**Pilote Bloc** :
```rust
pub trait BlockDevice: Send + Sync {
    fn read_sectors(&mut self, sector: u64, count: u64, 
                    data: *mut u8) -> Result<(), BlockError>;
    fn write_sectors(&mut self, sector: u64, count: u64, 
                     data: *const u8) -> Result<(), BlockError>;
    fn sector_size(&self) -> u64;
    fn total_sectors(&self) -> u64;
}
```

**Types supportés** :
- ATA/IDE (futur)
- AHCI/SATA (futur)
- NVMe (futur)
- RAM Disk (implémenté pour tests)

### libutils

**Rôle** : Utilitaires réutilisables

#### display.rs

**Affichage VGA Text Mode** :
```rust
const BUFFER_ADDR: usize = 0xb8000;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;

pub fn write_str_at(row: usize, col: usize, s: &str) {
    let buf = BUFFER_ADDR as *mut u8;
    for (i, &byte) in s.as_bytes().iter().enumerate() {
        let idx = (row * WIDTH + col + i) * 2;
        unsafe {
            core::ptr::write_volatile(buf.add(idx), byte);
            core::ptr::write_volatile(buf.add(idx + 1), 0x0F); // White on black
        }
    }
}
```

**Couleurs** :
```rust
pub enum Color {
    Black = 0x0,      Blue = 0x1,       Green = 0x2,
    Cyan = 0x3,       Red = 0x4,        Magenta = 0x5,
    Brown = 0x6,      LightGray = 0x7,  DarkGray = 0x8,
    LightBlue = 0x9,  LightGreen = 0xA, LightCyan = 0xB,
    LightRed = 0xC,   LightMagenta = 0xD, Yellow = 0xE,
    White = 0xF,
}
```

#### sync/mutex.rs

**Spinlock Mutex** :
```rust
pub struct Mutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

impl<T> Mutex<T> {
    pub fn lock(&self) -> MutexGuard<T> {
        while self.locked.compare_exchange_weak(
            false, true, 
            Ordering::Acquire, 
            Ordering::Relaxed
        ).is_err() {
            core::hint::spin_loop();
        }
        MutexGuard { mutex: self }
    }
}
```

**Utilisé pour** :
- Protéger structures partagées
- Ordonnanceur, IPC, drivers
- Alternative à `spin::Mutex` (plus de contrôle)

### perf_counters

**Rôle** : Mesures de performance (RDTSC)

**Compteurs** :
```rust
pub enum Component {
    Vga,         // Affichage VGA
    Scheduler,   // Ordonnanceur
    Ipc,         // IPC send/recv
    Syscall,     // Dispatch syscalls
    Memory,      // Allocations
    Interrupts,  // Handlers IRQ
}

pub struct ComponentStats {
    component: Component,
    total_calls: u64,
    total_cycles: u64,
    min_cycles: u64,
    max_cycles: u64,
}
```

**Mesure** :
```rust
let start = rdtsc();
// ... code à mesurer ...
let end = rdtsc();
PERF_MANAGER.record(Component::Vga, end - start);
```

**RDTSC (Read Time-Stamp Counter)** :
```rust
#[inline(always)]
pub fn rdtsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}
```

**Rapport** :
```
========== SYNTHESE DE PERFORMANCE ==========
VGA: 4 appels, 124769141 cycles moyen (41589.714 µs)
Scheduler: 128 appels, 15234 cycles moyen (5.078 µs)
IPC: 56 appels, 8923 cycles moyen (2.974 µs)
==============================================
```

---

## Gestion Mémoire

### Layout Mémoire

```
0x0000000000000000
+------------------+
| BIOS/Reserved    | 0-1MB
| (Real Mode)      |
+------------------+ 0x00100000 (1 MB)
| Kernel Code      |
| .text            | Kernel ELF sections
| .rodata          |
| .data            |
| .bss             |
+------------------+ ~0x00500000 (5 MB)
| Kernel Heap      | 16 MB linked_list_allocator
| (Dynamic Alloc)  |
+------------------+ ~0x01500000 (21 MB)
| Frame Bitmap     | Bitmap pour frame allocator
+------------------+
| Free Memory      | Gérée par frame allocator
| (Frames 4 KiB)   |
+------------------+
| VGA Buffer       | 0xb8000 (text mode)
+------------------+
| Hardware MMIO    | Devices mémory-mapped
+------------------+
| Top of RAM       | 512 MB (QEMU default)
+------------------+ 0x20000000
```

### Allocation Physique (Frames)

**Frame** = 4 KiB (4096 bytes)

**Bitmap Allocator** :
- 1 bit par frame
- 512 MB RAM = 131072 frames
- Bitmap = 131072 bits = 16384 bytes = 16 KB

**Recherche frame libre** :
```rust
fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
    for (i, &byte) in self.bitmap.iter().enumerate() {
        if byte != 0xFF { // Pas tous occupés
            for bit in 0..8 {
                if byte & (1 << bit) == 0 {
                    let frame_num = i * 8 + bit;
                    self.bitmap[i] |= 1 << bit; // Marquer occupé
                    let phys_addr = self.base_addr + frame_num * 4096;
                    return Some(PhysFrame::containing_address(
                        PhysAddr::new(phys_addr as u64)
                    ));
                }
            }
        }
    }
    None
}
```

### Allocation Virtuelle (Pages)

**Page Table Hierarchy (x86_64)** :
```
Virtual Address (48 bits)
┌────────┬────────┬────────┬────────┬────────────────┐
│  PML4  │  PDP   │   PD   │   PT   │     Offset     │
│ 9 bits │ 9 bits │ 9 bits │ 9 bits │    12 bits     │
└────────┴────────┴────────┴────────┴────────────────┘
   ↓        ↓        ↓        ↓           ↓
  512     512      512      512        4096 bytes
 entries entries  entries  entries     (1 page)
```

**Niveaux** :
- **PML4** (Page Map Level 4) : 512 entrées → 512 GB
- **PDP** (Page Directory Pointer) : 512 entrées → 1 GB
- **PD** (Page Directory) : 512 entrées → 2 MB
- **PT** (Page Table) : 512 entrées → 4 KB (1 page)

**Flags** :
```rust
PageTableFlags::PRESENT     // Page présente en RAM
PageTableFlags::WRITABLE    // Lecture/écriture
PageTableFlags::USER_ACCESSIBLE // Userspace access
PageTableFlags::WRITE_THROUGH   // Cache policy
PageTableFlags::NO_CACHE        // Désactive cache (VGA)
PageTableFlags::ACCESSED        // CPU a lu page
PageTableFlags::DIRTY           // CPU a écrit page
PageTableFlags::HUGE_PAGE       // 2MB/1GB page
PageTableFlags::NO_EXECUTE      // NX bit (sécurité)
```

### Allocation Heap

**linked_list_allocator** :
```
Heap Start (0x4444_4444_0000)
+------------------+
| Block Header     | size=128, used=true
+------------------+
| Allocated Data   | 128 bytes
+------------------+
| Block Header     | size=256, used=false (libre)
+------------------+
| Free Space       | 256 bytes
+------------------+
| Block Header     | size=64, used=true
+------------------+
| Allocated Data   | 64 bytes
+------------------+
| ...              |
+------------------+
Heap End (0x4444_4444_0000 + 16 MB)
```

**Opérations** :
- `alloc(size)` → Cherche bloc libre ≥size, split si trop grand
- `dealloc(ptr)` → Marque bloc libre, merge avec voisins libres

---

## Architecture x86_64

### Modes CPU

**Mode Long (64-bit)** :
- Activé par boot.asm
- Adresses virtuelles 48-bit
- Registres 64-bit (RAX, RBX, ..., R8-R15)
- Paging obligatoire

**Rings de Protection** :
```
Ring 0 (Kernel)    ← Privilèges max (accès tout)
Ring 1 (Drivers)   ← Inutilisé sur x86_64
Ring 2 (Drivers)   ← Inutilisé sur x86_64
Ring 3 (User)      ← Privilèges min (isolé)
```

### Registres Importants

**Généraux (64-bit)** :
- RAX, RBX, RCX, RDX : Calculs
- RSI, RDI : Source/destination (memcpy)
- RBP : Base pointer (stack frame)
- RSP : Stack pointer (top of stack)
- R8-R15 : Registres additionnels

**Contrôle** :
- RIP : Instruction pointer (PC)
- RFLAGS : Flags (ZF, CF, OF, IF, ...)

**Segments** :
- CS : Code segment selector (GDT)
- DS, ES, FS, GS : Data segments
- SS : Stack segment

**Système** :
- CR0 : Contrôle (paging, mode protégé)
- CR3 : Adresse PML4 (page table root)
- CR4 : Features (PAE, PSE, PGE)

### Interruptions

**IDT (Interrupt Descriptor Table)** :
```rust
pub struct Idt {
    entries: [IdtEntry; 256],
}

pub struct IdtEntry {
    handler_addr: u64,      // Adresse handler
    gdt_selector: u16,      // Segment code
    ist_offset: u8,         // IST index (0-7)
    type_attr: u8,          // Type + DPL
    reserved: u16,
}
```

**Exceptions CPU (0-31)** :
```
0  = Divide Error
1  = Debug
2  = NMI
3  = Breakpoint
4  = Overflow
5  = Bound Range Exceeded
6  = Invalid Opcode
7  = Device Not Available
8  = Double Fault         ← IST[0] (stack dédiée)
9  = Coprocessor Segment Overrun
10 = Invalid TSS
11 = Segment Not Present
12 = Stack-Segment Fault
13 = General Protection Fault
14 = Page Fault           ← Très important (page non mappée)
...
```

**IRQ Matériels (32-47)** :
```
32 = Timer (PIT)          ← Ordonnancement
33 = Clavier (PS/2)
34 = Cascade (PIC2)
35 = COM2
36 = COM1
37 = LPT2
38 = Floppy
39 = LPT1
40-47 = IRQ8-15 (PIC2)
```

**Handler Exemple** :
```rust
#[no_mangle]
pub extern "x86-interrupt" fn timer_interrupt_handler(
    _stack_frame: InterruptStackFrame
) {
    // Ordonnanceur : yield si temps expiré
    scheduler::tick();
    
    // EOI (End Of Interrupt) au PIC
    unsafe {
        PICS.lock().notify_end_of_interrupt(32);
    }
}
```

### TSS (Task State Segment)

**Structure** :
```rust
pub struct TaskStateSegment {
    reserved_1: u32,
    privilege_stack_table: [VirtAddr; 3],  // RSP0-2
    reserved_2: u64,
    interrupt_stack_table: [VirtAddr; 7],  // IST1-7
    reserved_3: u64,
    reserved_4: u16,
    iomap_base: u16,
}
```

**IST (Interrupt Stack Table)** :
- IST[0] : Double Fault (évite stack overflow récursif)
- IST[1-6] : Autres exceptions critiques
- Permet d'avoir stacks dédiées par exception

---

## Ordonnanceur

### Algorithme

**Round-Robin Coopératif** :
```
Ready Queue: [T1, T2, T3, T4]
                ↓
            Schedule()
                ↓
         T1.run() → Yield
                ↓
         T2.run() → Yield
                ↓
         T3.run() → Yield
                ↓
         T4.run() → Yield
                ↓
        (Retour à T1)
```

**Pas de Préemption** :
- Threads doivent appeler `yield()` volontairement
- Pas de timer interrupt forçant context switch
- Futur : Préemption avec timer IRQ

### Context Switch

**Étapes** :
1. Sauvegarder registres thread actuel
2. Choisir prochain thread (ready queue)
3. Restaurer registres nouveau thread
4. Continuer exécution nouveau thread

**Code ASM** :
```nasm
context_switch:
    # Sauvegarder ancien contexte (RDI = *old)
    mov [rdi + 0x00], rax
    mov [rdi + 0x08], rbx
    mov [rdi + 0x10], rcx
    # ... tous les registres ...
    pushfq
    pop QWORD PTR [rdi + 0x80] # RFLAGS
    mov [rdi + 0x88], rsp      # RSP
    lea rax, [rip + .return]
    mov [rdi + 0x90], rax      # RIP
    
    # Restaurer nouveau contexte (RSI = *new)
    mov rax, [rsi + 0x00]
    mov rbx, [rsi + 0x08]
    # ... tous les registres ...
    mov rsp, [rsi + 0x88]      # RSP
    push QWORD PTR [rsi + 0x90] # RIP
    ret                        # Saute vers RIP
.return:
    ret
```

### API Ordonnanceur

```rust
// Créer thread
pub fn spawn(
    entry: fn() -> !,
    name: Option<String>
) -> ThreadId;

// Yield CPU
pub fn yield_now();

// Bloquer thread
pub fn block(tid: ThreadId);

// Débloquer thread
pub fn unblock(tid: ThreadId);

// Terminer thread
pub fn exit();
```

---

## Système IPC

### Canaux

**Types de Canaux** :
- **Bounded** : Capacité fixe (blocking si plein)
- **Unbounded** : Capacité illimitée (peut OOM)

**Opérations** :
```rust
// Créer canal
pub fn create_channel(name: &str, capacity: usize) 
    -> Arc<Mutex<Channel>>;

// Envoyer message
pub fn send(channel: &Arc<Mutex<Channel>>, msg: Message)
    -> Result<(), IpcError>;

// Recevoir message (bloquant)
pub fn recv(channel: &Arc<Mutex<Channel>>)
    -> Result<Message, IpcError>;

// Recevoir message (non-bloquant)
pub fn try_recv(channel: &Arc<Mutex<Channel>>)
    -> Result<Option<Message>, IpcError>;
```

### Messages

**Fast Path (≤64 bytes)** :
```rust
let msg = Message::new_fast(&data[..64]);
// Pas d'allocation heap, copié sur stack
```

**Slow Path (>64 bytes)** :
```rust
let msg = Message::new_large(data);
// Allocation Vec sur heap
```

**Shared Memory** :
```rust
let msg = Message::new_shared(ptr, size);
// Juste échange de pointeurs, pas de copie
```

### Synchronisation

**Blocking Send** :
```
Thread T1 : send(full_channel, msg)
    ↓
Channel plein, T1 bloqué
    ↓
Ajoute T1 à blocked_senders
    ↓
Yield CPU (context switch)
    ↓
Thread T2 : recv(channel)
    ↓
Channel a de la place
    ↓
Débloquer T1 (ready queue)
    ↓
T1 reprend, send réussit
```

---

## Appels Système

### Convention Appel

**x86_64 System V ABI** :
```
Syscall Number: RAX
Arg1: RDI
Arg2: RSI
Arg3: RDX
Arg4: R10
Arg5: R8
Arg6: R9
Return: RAX
```

**Instruction** :
```asm
mov rax, 1      # sys_write
mov rdi, 1      # fd = stdout
mov rsi, buffer # buf
mov rdx, len    # count
syscall         # Appel kernel
```

### Implémentation Kernel

**Handler Syscall** :
```rust
#[no_mangle]
pub extern "C" fn syscall_handler() {
    let args = SyscallArgs::from_registers();
    let result = dispatch_syscall(&args);
    set_return_value(result);
}
```

**Dispatch** :
```rust
pub fn dispatch_syscall(args: &SyscallArgs) -> u64 {
    match args.rax {
        0 => sys_exit(args.rdi as i32),
        1 => sys_read(args.rdi, args.rsi as *mut u8, args.rdx),
        2 => sys_write(args.rdi, args.rsi as *const u8, args.rdx),
        // ...
        _ => ENOSYS, // Syscall non implémenté
    }
}
```

### Sécurité

**Validations** :
1. Vérifier pointeurs userspace
2. Vérifier tailles buffers
3. Vérifier permissions (fichiers, mémoire)
4. Limiter ressources (descripteurs, mémoire)

**Exemple** :
```rust
pub fn sys_write(fd: u64, buf: *const u8, count: u64) -> u64 {
    // 1. Vérifier fd valide
    if fd > MAX_FD {
        return EBADF;
    }
    
    // 2. Vérifier pointeur userspace
    if !is_userspace_ptr(buf) {
        return EFAULT;
    }
    
    // 3. Vérifier taille raisonnable
    if count > MAX_WRITE_SIZE {
        return EINVAL;
    }
    
    // 4. Effectuer opération
    // ...
}
```

---

## Pilotes

### Architecture Pilotes

```
Userspace
    ↓ syscall
Kernel Syscall Layer
    ↓ dispatch
Driver Manager
    ↓ get_driver(type)
Pilote Spécifique
    ↓ read/write
Matériel
```

### Enregistrement Pilote

```rust
// Dans driver::init()
let serial = Arc::new(Mutex::new(SerialDriver::new(0x3F8)));
DRIVER_MANAGER.lock().register_driver(serial)?;

let ramdisk = Arc::new(Mutex::new(RamDiskDriver::new(1024)));
DRIVER_MANAGER.lock().register_driver(ramdisk)?;
```

### Accès Pilote

```rust
// Depuis syscall
pub fn sys_read(fd: u64, buf: *mut u8, count: u64) -> u64 {
    let file = get_file(fd)?;
    let driver_id = file.driver_id;
    
    let manager = DRIVER_MANAGER.lock();
    let driver = manager.get_driver(driver_id)?;
    
    let mut d = driver.lock();
    d.read(buf, count)
}
```

---

## Performances

### Métriques Clés

**Boot Time** :
- **Objectif** : <800 ms
- **Actuel** : ~2-3 secondes (debug), ~500 ms (release)
- **Optimisations** : Lazy init, parallélisation, LTO

**Binary Size** :
- **Objectif** : <3 MB
- **Actuel** : ~5 MB (debug), ~2 MB (release stripped)
- **Optimisations** : Strip symbols, LTO, opt-level

**Memory Usage** :
- **Objectif** : <64 MB RAM
- **Actuel** : 16 MB heap, ~20 MB total
- **Optimisations** : Réduire heap, lazy allocation

### Compteurs Performance

**RDTSC (Read Time-Stamp Counter)** :
- Compte cycles CPU depuis boot
- Résolution : 1 cycle (~0.3 ns @ 3 GHz)
- Overhead : ~30 cycles (~10 ns)

**Conversion Cycles → Temps** :
```rust
let cycles = 3_000_000_000; // 3 milliards cycles
let freq_ghz = 3.0;          // 3 GHz CPU
let time_s = cycles as f64 / (freq_ghz * 1e9);
let time_ms = time_s * 1000.0;
let time_us = time_s * 1e6;
```

**Mesures Typiques** :
```
VGA write_banner:   124M cycles (41 ms)  ← LENT !
Context switch:     15K cycles (5 µs)
IPC send/recv:      9K cycles (3 µs)
Syscall dispatch:   2K cycles (0.7 µs)
Mutex lock/unlock:  50 cycles (17 ns)
```

---

## Configuration Build

### Cargo.toml (Workspace)

```toml
[workspace]
members = ["kernel"]

[profile.dev]
opt-level = 0              # Pas d'optimisation
debug = true               # Symboles debug
lto = false                # Pas de LTO

[profile.release]
opt-level = "z"            # Optimise taille (pas vitesse)
debug = false              # Pas de symboles
strip = true               # Strip binaire
lto = "fat"                # LTO agressif
codegen-units = 1          # 1 seul codegen (+ lent build, + rapide runtime)
panic = "abort"            # Pas d'unwinding
overflow-checks = false    # Pas de checks overflow
```

### Cargo.toml (Kernel)

```toml
[package]
name = "exo-kernel"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["staticlib"]  # Bibliothèque statique

[[bin]]
name = "exo-kernel"
path = "src/main.rs"

[dependencies]
x86_64 = "0.14.13"
spin = "0.9.8"
# ... (voir section Vue d'Ensemble)

[build-dependencies]
cc = "1.2.41"  # Compiler C/ASM
```

### .cargo/config.toml

```toml
[build]
target = "x86_64-unknown-none"

[target.x86_64-unknown-none]
linker = "rust-lld"
rustflags = [
    "-C", "link-arg=-Tc:/Users/Eric/Documents/Exo-OS/linker.ld",
    "-C", "link-arg=--strip-debug",
    "-C", "link-arg=-z", "link-arg=norelro",     # Pas de RELRO
    "-C", "link-arg=-z", "link-arg=now",          # Résolution immédiate
    "-C", "link-arg=--gc-sections",               # Supprime sections inutilisées
    "-C", "link-arg=-O1",                         # Optimisation linker
]

[unstable]
build-std-features = ["compiler-builtins-mem"]
build-std = ["core", "compiler_builtins", "alloc"]
```

### linker.ld

```ld
OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS {
    . = 0x100000;  /* 1 MiB - après BIOS */

    _kernel_start = .;

    .boot : ALIGN(16) {
        KEEP(*(.multiboot_header))  /* Header Multiboot2 */
        KEEP(*(.text.boot))         /* Code boot ASM/C */
    }

    .text : ALIGN(16) {
        *(.text .text.*)            /* Code Rust */
    }

    .rodata : ALIGN(16) {
        *(.rodata .rodata.*)        /* Constantes */
    }

    .data : ALIGN(16) {
        *(.data .data.*)            /* Données initialisées */
    }

    .bss (NOLOAD) : ALIGN(16) {
        _bss_start = .;
        *(.bss .bss.*)              /* Données non-initialisées */
        *(COMMON)
        _bss_end = .;
    }

    _kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame)                /* Pas d'unwinding */
        *(.eh_frame_hdr)
        *(.comment)
        *(.note .note.*)
        *(.gcc_except_table)
        *(.gnu.hash)
    }
}
```

### x86_64-unknown-none.json

```json
{
  "llvm-target": "x86_64-unknown-none",
  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128",
  "arch": "x86_64",
  "target-endian": "little",
  "target-pointer-width": "64",
  "target-c-int-width": "32",
  "os": "none",
  "executables": true,
  "linker-flavor": "ld.lld",
  "linker": "rust-lld",
  "panic-strategy": "abort",
  "disable-redzone": true,
  "features": "-mmx,-sse,+soft-float",
  "code-model": "kernel",
  "relocation-model": "static"
}
```

**Explications** :
- `disable-redzone` : Pas de red zone (128 bytes sous RSP)
- `-mmx,-sse,+soft-float` : Pas d'instructions SIMD (kernel)
- `code-model=kernel` : Adresses haute mémoire
- `relocation-model=static` : Pas de PIC (Position Independent Code)

### build.rs

```rust
use std::env;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap();
    
    // Compiler context_switch.S uniquement pour x86_64-unknown-none
    if target == "x86_64-unknown-none" {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        
        // Compiler ASM avec gcc
        cc::Build::new()
            .file("src/scheduler/context_switch.S")
            .flag("-c")
            .compile("context_switch");
        
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=context_switch");
    }
}
```

---

## Annexes

### Commandes Utiles

**Compilation** :
```bash
# Debug
cargo build --target x86_64-unknown-none

# Release
cargo build --target x86_64-unknown-none --release

# Nettoyage
cargo clean
```

**ISO Bootable** :
```bash
wsl bash -lc "cd /mnt/c/Users/Eric/Documents/Exo-OS && ./scripts/build-iso.sh"
```

**Lancement QEMU** :
```bash
wsl bash -lc "cd /mnt/c/Users/Eric/Documents/Exo-OS && ./scripts/run-qemu.sh"
```

**Taille Binaire** :
```bash
ls -lh target/x86_64-unknown-none/debug/exo-kernel
ls -lh target/x86_64-unknown-none/release/exo-kernel
```

**Symboles** :
```bash
nm target/x86_64-unknown-none/release/exo-kernel | grep " T "
```

**Sections** :
```bash
objdump -h target/x86_64-unknown-none/release/exo-kernel
```

### Problèmes Courants

**1. Linker Error: cannot find linker script**
- **Cause** : Chemin WSL (`/mnt/c/...`) vs Windows (`c:/...`)
- **Solution** : Utiliser `c:/...` dans `.cargo/config.toml`

**2. Double Fault au Boot**
- **Cause** : Stack overflow, mauvaise init GDT/IDT
- **Solution** : Vérifier IST, augmenter stack size

**3. Page Fault**
- **Cause** : Accès mémoire non mappée (ex: VGA 0xb8000)
- **Solution** : Vérifier page tables, mapper région

**4. c_compat not found**
- **Cause** : `build.rs` ne compile pas C, mais `lib.rs` l'importe
- **Solution** : Commenter `pub mod c_compat;` ou activer build C

**5. Écran Noir (VGA)**
- **Cause** : QEMU ferme fenêtre immédiatement
- **Solution** : Ajouter `-no-shutdown -no-reboot` à QEMU

### Ressources

**Documentation** :
- [OSDev Wiki](https://wiki.osdev.org/)
- [Intel Manual Vol. 3](https://software.intel.com/content/www/us/en/develop/articles/intel-sdm.html)
- [x86_64 Crate Docs](https://docs.rs/x86_64/)
- [Rust Embedded Book](https://rust-embedded.github.io/book/)

**Tutoriels** :
- [Writing an OS in Rust](https://os.phil-opp.com/)
- [Bare Metal Rust](https://www.rust-lang.org/what/embedded)

**Outils** :
- QEMU : Émulateur
- GDB : Debugger
- objdump : Analyse binaire
- nm : Symboles

---

**Fin de la Documentation Architecture Noyau**

Version: 1.0  
Auteur: Équipe Exo-OS  
Date: 10 novembre 2025
