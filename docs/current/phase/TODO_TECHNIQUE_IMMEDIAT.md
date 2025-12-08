# 🔧 TODO TECHNIQUE v1.0.0 - Actions Immédiates

**Date:** 3 décembre 2025  
**Objectif:** Transformer les stubs en implémentations réelles

---

## 🔴 PRIORITÉ 0 - CETTE SEMAINE

### 1. Timer Preemption (Bloquant!)

**Fichier:** `kernel/src/arch/x86_64/interrupts.rs` (ou similaire)

**Problème actuel:** Le scheduler a un context switch fonctionnel mais il n'est jamais appelé automatiquement.

**Solution:**
```rust
// À ajouter dans le timer interrupt handler
pub extern "x86-interrupt" fn timer_handler(frame: InterruptStackFrame) {
    // 1. Incrémenter tick counter
    crate::time::tick();
    
    // 2. Preemption check (tous les 10 ticks = 100ms à 100Hz)
    static PREEMPT_COUNTER: AtomicU32 = AtomicU32::new(0);
    let count = PREEMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    
    if count % 10 == 0 {
        // 3. Appeler le scheduler
        crate::scheduler::core::scheduler::SCHEDULER.schedule();
    }
    
    // 4. EOI (End Of Interrupt)
    unsafe {
        crate::arch::x86_64::pic::end_of_interrupt(0x20);
    }
}
```

**Test:** Spawner 2 threads qui printent et vérifier qu'ils alternent.

---

### 2. map_page() / unmap_page() (Bloquant!)

**Fichier:** `kernel/src/memory/virtual_mem/mapper.rs` (à créer/compléter)

**Code requis:**
```rust
use crate::memory::address::{PhysicalAddress, VirtualAddress};
use crate::memory::physical::frame_allocator;

pub struct PageMapper;

impl PageMapper {
    /// Map une page virtuelle vers une page physique
    pub fn map_page(
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: PageFlags,
    ) -> Result<(), MapError> {
        // 1. Obtenir les indices de page table
        let p4_idx = (virt.as_usize() >> 39) & 0x1FF;
        let p3_idx = (virt.as_usize() >> 30) & 0x1FF;
        let p2_idx = (virt.as_usize() >> 21) & 0x1FF;
        let p1_idx = (virt.as_usize() >> 12) & 0x1FF;
        
        // 2. Naviguer/créer les niveaux de page table
        let p4 = get_p4_table();
        let p3 = get_or_create_next_table(&mut p4[p4_idx])?;
        let p2 = get_or_create_next_table(&mut p3[p3_idx])?;
        let p1 = get_or_create_next_table(&mut p2[p2_idx])?;
        
        // 3. Mapper la page finale
        if p1[p1_idx].is_present() {
            return Err(MapError::AlreadyMapped);
        }
        
        p1[p1_idx] = PageTableEntry::new(phys, flags);
        
        // 4. Flush TLB pour cette adresse
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) virt.as_usize());
        }
        
        Ok(())
    }
    
    /// Unmap une page virtuelle
    pub fn unmap_page(virt: VirtualAddress) -> Result<PhysicalAddress, MapError> {
        let p4_idx = (virt.as_usize() >> 39) & 0x1FF;
        let p3_idx = (virt.as_usize() >> 30) & 0x1FF;
        let p2_idx = (virt.as_usize() >> 21) & 0x1FF;
        let p1_idx = (virt.as_usize() >> 12) & 0x1FF;
        
        let p4 = get_p4_table();
        let p3 = get_next_table(&p4[p4_idx])?;
        let p2 = get_next_table(&p3[p3_idx])?;
        let p1 = get_next_table(&p2[p2_idx])?;
        
        if !p1[p1_idx].is_present() {
            return Err(MapError::NotMapped);
        }
        
        let phys = p1[p1_idx].physical_address();
        p1[p1_idx] = PageTableEntry::empty();
        
        // Flush TLB
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) virt.as_usize());
        }
        
        Ok(phys)
    }
}

fn get_or_create_next_table(entry: &mut PageTableEntry) -> Result<&mut PageTable, MapError> {
    if !entry.is_present() {
        // Allouer une nouvelle frame pour la page table
        let frame = frame_allocator::allocate_frame()
            .ok_or(MapError::OutOfMemory)?;
        
        // Initialiser à zéro
        let table_ptr = frame.as_usize() as *mut PageTable;
        unsafe { core::ptr::write_bytes(table_ptr, 0, 1); }
        
        // Créer l'entrée
        *entry = PageTableEntry::new(
            PhysicalAddress::new(frame.as_usize()),
            PageFlags::PRESENT | PageFlags::WRITABLE,
        );
    }
    
    let table_addr = entry.physical_address().as_usize();
    Ok(unsafe { &mut *(table_addr as *mut PageTable) })
}
```

---

### 3. pipe() syscall (Important pour shell)

**Fichier:** `kernel/src/posix_x/syscalls/hybrid_path/pipe.rs` (à créer)

```rust
//! Pipe syscall implementation

use crate::ipc::fusion_ring::FusionRing;
use alloc::sync::Arc;
use spin::RwLock;

/// Pipe structure using Fusion Ring for high performance
pub struct Pipe {
    ring: FusionRing,
    read_open: bool,
    write_open: bool,
}

impl Pipe {
    pub fn new() -> Self {
        Self {
            ring: FusionRing::new(4096), // 4KB buffer
            read_open: true,
            write_open: true,
        }
    }
    
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, PipeError> {
        if !self.read_open {
            return Err(PipeError::Closed);
        }
        
        match self.ring.recv(buf) {
            Ok(n) => Ok(n),
            Err(_) if !self.write_open => Ok(0), // EOF
            Err(e) => Err(PipeError::Io),
        }
    }
    
    pub fn write(&self, buf: &[u8]) -> Result<usize, PipeError> {
        if !self.write_open {
            return Err(PipeError::Closed);
        }
        if !self.read_open {
            return Err(PipeError::BrokenPipe); // SIGPIPE
        }
        
        match self.ring.send(buf) {
            Ok(()) => Ok(buf.len()),
            Err(e) => Err(PipeError::Io),
        }
    }
}

/// sys_pipe - Create a pipe
pub fn sys_pipe(pipefd: *mut [i32; 2]) -> i64 {
    if pipefd.is_null() {
        return -libc::EFAULT as i64;
    }
    
    let pipe = Arc::new(RwLock::new(Pipe::new()));
    
    // Créer deux file descriptors
    let read_handle = VfsHandle::pipe_read(pipe.clone());
    let write_handle = VfsHandle::pipe_write(pipe);
    
    let mut fd_table = GLOBAL_FD_TABLE.write();
    let read_fd = fd_table.allocate(read_handle)?;
    let write_fd = fd_table.allocate(write_handle)?;
    
    unsafe {
        (*pipefd)[0] = read_fd;
        (*pipefd)[1] = write_fd;
    }
    
    0
}
```

---

## 🟠 PRIORITÉ 1 - CETTE SEMAINE/PROCHAINE

### 4. fork() - Clone de processus

**Fichier:** `kernel/src/posix_x/syscalls/legacy_path/fork.rs`

**Remplacer le stub actuel:**
```rust
// ACTUEL (stub):
pub fn sys_fork() -> i64 {
    -38 // ENOSYS
}

// NOUVEAU (implémentation):
pub fn sys_fork() -> i64 {
    let current_thread = match SCHEDULER.current_thread_id() {
        Some(tid) => tid,
        None => return -libc::ESRCH as i64,
    };
    
    // 1. Allouer nouveau PID
    let child_pid = alloc_pid();
    
    // 2. Créer ProcessControlBlock pour l'enfant
    let mut child_pcb = ProcessControlBlock::new(child_pid);
    child_pcb.parent_pid = Some(get_current_pid());
    
    // 3. Clone le thread context
    let child_context = SCHEDULER.with_current_thread(|t| {
        let mut ctx = t.context().clone();
        ctx.set_return_value(0); // L'enfant reçoit 0
        ctx
    }).unwrap();
    
    // 4. Clone l'address space (Copy-on-Write)
    // Pour l'instant, version simple sans CoW
    let child_mm = clone_address_space_simple()?;
    child_pcb.mm = child_mm;
    
    // 5. Clone les file descriptors
    child_pcb.files = clone_fd_table();
    
    // 6. Clone les signal handlers
    child_pcb.signals = clone_signal_handlers();
    
    // 7. Créer le thread enfant
    let child_thread = Thread::new_forked(
        alloc_thread_id(),
        &format!("forked-{}", child_pid),
        child_context,
    );
    
    // 8. Enregistrer le PCB
    PROCESS_TABLE.write().insert(child_pid, child_pcb);
    
    // 9. Ajouter au scheduler
    SCHEDULER.add_thread(child_thread);
    
    // 10. Le parent reçoit le PID de l'enfant
    child_pid as i64
}
```

---

### 5. exec() - Charger et exécuter ELF

**Fichier:** `kernel/src/posix_x/syscalls/legacy_path/exec.rs`

```rust
pub fn sys_execve(pathname: usize, argv: usize, envp: usize) -> i64 {
    // 1. Lire le path
    let path = match read_user_string(pathname) {
        Ok(s) => s,
        Err(_) => return -libc::EFAULT as i64,
    };
    
    // 2. Lire les arguments et environnement
    let args = read_user_string_array(argv)?;
    let env = read_user_string_array(envp)?;
    
    // 3. Lire le fichier ELF depuis VFS
    let elf_data = match crate::fs::vfs::read_file(&path) {
        Ok(data) => data,
        Err(_) => return -libc::ENOENT as i64,
    };
    
    // 4. Valider l'ELF
    let loaded = match load_elf(&elf_data, None) {
        Ok(l) => l,
        Err(_) => return -libc::ENOEXEC as i64,
    };
    
    // 5. Remplacer l'address space
    let current = get_current_process();
    current.mm.clear(); // Détruire l'ancien
    
    // 6. Mapper les segments ELF
    for segment in &loaded.segments {
        let data = &elf_data[segment.data_offset..segment.data_offset + segment.file_size];
        current.mm.map_segment(segment, data)?;
    }
    
    // 7. Créer la stack utilisateur
    let stack_top = VirtualAddress::new(0x7FFF_FFFF_F000);
    let stack_size = 8 * 1024 * 1024; // 8MB
    current.mm.map_stack(stack_top, stack_size)?;
    
    // 8. Setup stack avec argv/envp/auxv
    let sp = setup_user_stack(stack_top, &args, &env, &loaded)?;
    
    // 9. Reset signals
    current.signals.reset_on_exec();
    
    // 10. Fermer les FDs avec O_CLOEXEC
    current.files.close_on_exec();
    
    // 11. Préparer le retour en userspace
    // execve ne retourne pas en cas de succès
    // On modifie le context pour sauter à l'entry point
    SCHEDULER.with_current_thread(|t| {
        let ctx = t.context_mut();
        ctx.rip = loaded.entry_point.value() as u64;
        ctx.rsp = sp.value() as u64;
        ctx.rdi = args.len() as u64; // argc
        // argv, envp seront sur la stack
    });
    
    // Ce code ne sera jamais atteint car on va directement
    // à l'entry point après le return de syscall
    0
}
```

---

### 6. Clavier PS/2 Driver

**Fichier:** `kernel/src/drivers/input/keyboard.rs` (à créer/compléter)

```rust
//! PS/2 Keyboard Driver

use crate::arch::x86_64::io::{inb, outb};
use spin::Mutex;

const PS2_DATA: u16 = 0x60;
const PS2_STATUS: u16 = 0x64;
const PS2_COMMAND: u16 = 0x64;

/// Circular buffer for keyboard input
struct KeyboardBuffer {
    data: [u8; 256],
    head: usize,
    tail: usize,
}

impl KeyboardBuffer {
    const fn new() -> Self {
        Self {
            data: [0; 256],
            head: 0,
            tail: 0,
        }
    }
    
    fn push(&mut self, c: u8) {
        let next = (self.head + 1) % 256;
        if next != self.tail {
            self.data[self.head] = c;
            self.head = next;
        }
    }
    
    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            None
        } else {
            let c = self.data[self.tail];
            self.tail = (self.tail + 1) % 256;
            Some(c)
        }
    }
}

static KEYBOARD_BUFFER: Mutex<KeyboardBuffer> = Mutex::new(KeyboardBuffer::new());

/// Scancode to ASCII (US layout, simplified)
const SCANCODE_TO_ASCII: [u8; 128] = [
    0, 27, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b'-', b'=', 8,
    b'\t', b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n',
    0, b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'', b'`',
    0, b'\\', b'z', b'x', b'c', b'v', b'b', b'n', b'm', b',', b'.', b'/', 0,
    b'*', 0, b' ', // ... reste
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Initialize keyboard driver
pub fn init() {
    // Enable IRQ1
    unsafe {
        // Read current mask
        let mask = inb(0x21);
        // Clear bit 1 (enable keyboard)
        outb(0x21, mask & !0x02);
    }
    
    log::info!("PS/2 keyboard driver initialized");
}

/// Keyboard interrupt handler (IRQ1)
pub fn handle_interrupt() {
    let scancode = unsafe { inb(PS2_DATA) };
    
    // Ignorer les key release (bit 7 set)
    if scancode & 0x80 == 0 {
        if let Some(ascii) = scancode_to_char(scancode) {
            KEYBOARD_BUFFER.lock().push(ascii);
        }
    }
}

fn scancode_to_char(scancode: u8) -> Option<u8> {
    if (scancode as usize) < SCANCODE_TO_ASCII.len() {
        let c = SCANCODE_TO_ASCII[scancode as usize];
        if c != 0 {
            return Some(c);
        }
    }
    None
}

/// Read a character (blocking)
pub fn read_char() -> u8 {
    loop {
        if let Some(c) = KEYBOARD_BUFFER.lock().pop() {
            return c;
        }
        // Yield to other threads while waiting
        crate::scheduler::yield_now();
    }
}

/// Read a character (non-blocking)
pub fn try_read_char() -> Option<u8> {
    KEYBOARD_BUFFER.lock().pop()
}
```

---

## 🟡 PRIORITÉ 2 - SEMAINES SUIVANTES

### 7. devfs - /dev/null, /dev/zero, /dev/console

```rust
// kernel/src/fs/devfs/mod.rs

pub struct DevFs {
    devices: BTreeMap<String, Arc<dyn CharDevice>>,
}

impl DevFs {
    pub fn init() -> Self {
        let mut devfs = Self { devices: BTreeMap::new() };
        
        devfs.register("null", Arc::new(DevNull));
        devfs.register("zero", Arc::new(DevZero));
        devfs.register("console", Arc::new(DevConsole));
        devfs.register("tty", Arc::new(DevTty));
        
        devfs
    }
}

struct DevNull;
impl CharDevice for DevNull {
    fn read(&self, _buf: &mut [u8]) -> usize { 0 } // EOF
    fn write(&self, buf: &[u8]) -> usize { buf.len() } // Discard
}

struct DevZero;
impl CharDevice for DevZero {
    fn read(&self, buf: &mut [u8]) -> usize {
        buf.fill(0);
        buf.len()
    }
    fn write(&self, buf: &[u8]) -> usize { buf.len() }
}

struct DevConsole;
impl CharDevice for DevConsole {
    fn read(&self, buf: &mut [u8]) -> usize {
        // Read from keyboard
        let c = keyboard::read_char();
        buf[0] = c;
        1
    }
    fn write(&self, buf: &[u8]) -> usize {
        // Write to serial + VGA
        for &b in buf {
            serial::write_byte(b);
            vga::write_byte(b);
        }
        buf.len()
    }
}
```

---

### 8. Benchmark Infrastructure

**Fichier:** `kernel/src/benchmark/mod.rs`

```rust
//! Kernel benchmarking infrastructure

use core::sync::atomic::{AtomicU64, Ordering};

/// Read TSC (Time Stamp Counter)
#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Benchmark results
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub name: &'static str,
    pub min_cycles: u64,
    pub max_cycles: u64,
    pub avg_cycles: u64,
    pub iterations: u64,
    pub target: u64,
    pub passed: bool,
}

/// Run a benchmark
pub fn benchmark<F>(name: &'static str, target: u64, iterations: u64, mut f: F) -> BenchResult
where
    F: FnMut(),
{
    let mut min = u64::MAX;
    let mut max = 0u64;
    let mut total = 0u64;
    
    // Warmup
    for _ in 0..100 {
        f();
    }
    
    // Actual benchmark
    for _ in 0..iterations {
        let start = rdtsc();
        f();
        let elapsed = rdtsc() - start;
        
        min = min.min(elapsed);
        max = max.max(elapsed);
        total += elapsed;
    }
    
    let avg = total / iterations;
    
    BenchResult {
        name,
        min_cycles: min,
        max_cycles: max,
        avg_cycles: avg,
        iterations,
        target,
        passed: avg <= target,
    }
}

/// Benchmark IPC (Fusion Rings)
pub fn bench_ipc() -> BenchResult {
    use crate::ipc::fusion_ring::FusionRing;
    
    let ring = FusionRing::new(256);
    let mut buffer = [0u8; 64];
    let msg = b"test message for IPC benchmark";
    
    benchmark("IPC Roundtrip", 347, 10000, || {
        let _ = ring.send(msg);
        let _ = ring.recv(&mut buffer);
    })
}

/// Benchmark Context Switch
pub fn bench_context_switch() -> BenchResult {
    benchmark("Context Switch", 304, 1000, || {
        crate::scheduler::yield_now();
    })
}

/// Benchmark Allocator
pub fn bench_allocator() -> BenchResult {
    benchmark("Alloc 64B", 8, 10000, || {
        let ptr = alloc::alloc::alloc(
            alloc::alloc::Layout::from_size_align(64, 8).unwrap()
        );
        unsafe { alloc::alloc::dealloc(ptr, 
            alloc::alloc::Layout::from_size_align(64, 8).unwrap()); }
    })
}

/// Run all benchmarks and print results
pub fn run_all_benchmarks() {
    log::info!("=== KERNEL BENCHMARKS ===");
    
    let results = [
        bench_ipc(),
        bench_context_switch(),
        bench_allocator(),
    ];
    
    for r in &results {
        let status = if r.passed { "✅ PASS" } else { "❌ FAIL" };
        log::info!(
            "{}: {} avg={} min={} max={} target={}",
            status,
            r.name,
            r.avg_cycles,
            r.min_cycles,
            r.max_cycles,
            r.target
        );
    }
    
    let passed = results.iter().filter(|r| r.passed).count();
    log::info!("=== {}/{} BENCHMARKS PASSED ===", passed, results.len());
}
```

---

## 📊 CHECKLIST DE PROGRESSION

### Semaine 1
- [ ] Timer preemption fonctionne
- [ ] 2 threads alternent visiblement
- [ ] map_page() basique fonctionne
- [ ] Benchmark context switch implémenté

### Semaine 2
- [ ] unmap_page() fonctionne
- [ ] Page fault handler basique
- [ ] pipe() syscall fonctionne
- [ ] Keyboard driver reçoit input

### Semaine 3
- [ ] fork() crée un processus enfant
- [ ] Le processus enfant s'exécute
- [ ] devfs avec /dev/null, /dev/console

### Semaine 4
- [ ] exec() charge un ELF simple
- [ ] Premier programme userspace ("hello")
- [ ] Shell basique qui lit/écrit

---

## 🚀 COMMANDES DE TEST

```bash
# Build complet
cd /workspaces/Exo-OS && ./scripts/build_complete.sh

# Test QEMU avec serial
qemu-system-x86_64 -cdrom build/exo_os.iso -m 256M -serial stdio

# Test avec log détaillé
qemu-system-x86_64 -cdrom build/exo_os.iso -m 256M \
    -serial file:serial.log -d int,cpu_reset -D qemu.log

# Voir les logs
tail -f serial.log

# Debug GDB
qemu-system-x86_64 -cdrom build/exo_os.iso -m 256M -s -S &
gdb build/kernel.elf -ex "target remote :1234"
```

---

**🎯 Focus immédiat: Timer preemption + map_page()**

*Une chose à la fois, bien faite.*
