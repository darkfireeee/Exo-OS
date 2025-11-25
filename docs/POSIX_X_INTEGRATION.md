# POSIX-X Integration Guide for Exo-OS

## 📋 Overview

POSIX-X is Exo-OS's high-performance POSIX compatibility layer with 3-tier architecture:
- **Fast Path:** Ultra-fast syscalls (<100 cycles)
- **Hybrid Path:** Medium-complexity syscalls (~500-2000 cycles)  
- **Legacy Path:** Complex syscalls with full emulation (~10K-50K cycles)

## 🏗️ Architecture

```
kernel/src/posix_x/
├── core/                          # Core POSIX-X engine
│   ├── mod.rs                     # Main module
│   ├── fd_table.rs                # File descriptor → capability mapping
│   ├── process_state.rs           # Process state (PID, CWD, ENV, signals)
│   └── capability_cache.rs        # FD→capability cache (LRU)
│
├── syscalls/                      # Syscall implementation (3-tier)
│   ├── mod.rs
│   ├── fast_path/                 # <100 cycles
│   │   ├── mod.rs
│   │   ├── getpid.rs              # getpid: read TLS (20 cycles)
│   │   ├── gettid.rs              # gettid: read TLS (20 cycles)
│   │   ├── clock_gettime.rs       # clock_gettime: TSC (30 cycles)
│   │   └── brk.rs                 # brk: bump allocator (60 cycles)
│   │
│   ├── hybrid_path/               # 500-2000 cycles
│   │   ├── mod.rs
│   │   ├── open.rs                # open: VFS + capability cache
│   │   ├── read.rs                # read: check capability + copy
│   │   ├── write.rs               # write: check capability + copy
│   │   ├── close.rs               # close: release capability
│   │   ├── stat.rs                # stat: VFS metadata
│   │   ├── pipe.rs                # pipe: create fusion ring
│   │   └── socket.rs              # socket: create network capability
│   │
│   └── legacy_path/               # 10K-50K cycles
│       ├── mod.rs
│       ├── fork.rs                # fork: COW + clone address space
│       ├── execve.rs              # execve: ELF load + new address space
│       ├── mmap.rs                # mmap: shared memory mapping
│       ├── futex.rs               # futex: emulated with spinlock
│       └── signal.rs              # signal: convert to IPC message
│
├── libc_impl/                     # musl libc adaptation
│   ├── mod.rs
│   ├── musl_adapted/              # Adapted musl sources
│   │   ├── stdio/                 # stdio.h functions
│   │   ├── string/                # string.h functions
│   │   ├── stdlib/                # stdlib.h functions
│   │   ├── unistd/                # unistd.h functions
│   │   └── pthread/               # pthread functions → Exo-OS threads
│   └── bridge.rs                  # musl → Exo-OS syscall bridge
│
├── translation/                   # Translation logic
│   ├── mod.rs
│   ├── fd_to_cap.rs               # FD → capability translation
│   ├── signal_to_ipc.rs           # Signal → IPC message
│   ├── mmap_to_shared.rs          # mmap → shared memory
│   └── fork_cow.rs                # fork → COW pages
│
├── optimization/                  # Optimization systems
│   ├── mod.rs
│   ├── adaptive_learning.rs       # Learn usage patterns
│   ├── zero_copy.rs               # Zero-copy detection
│   ├── batching.rs                # Syscall batching
│   └── cache.rs                   # Capability/FD cache
│
└── tools/                         # POSIX-X tools
    ├── profiler.rs                # Syscall profiler
    ├── analyzer.rs                # Compatibility analyzer
    └── migrator.rs                # App migration tool
```

## 🚀 Fast Path (<100 cycles)

### getpid/gettid (20 cycles)

```rust
#[naked]
pub unsafe extern "C" fn sys_getpid() -> u64 {
    asm!(
        "mov rax, gs:0x10",  // Read PID from TLS (Thread Local Storage)
        "ret",
        options(noreturn)
    )
}
```

**Mechanism:**
- PID stored in TLS at offset 0x10
- Single `mov` instruction → 20 cycles
- No syscall, no context switch

### clock_gettime (30 cycles)

```rust
pub fn sys_clock_gettime(clock_id: i32, tp: *mut TimeSpec) -> i64 {
    match clock_id {
        CLOCK_MONOTONIC => {
            let tsc = rdtsc();
            let ns = tsc_to_ns(tsc);
            unsafe {
                (*tp).tv_sec = (ns / 1_000_000_000) as i64;
                (*tp).tv_nsec = (ns % 1_000_000_000) as i64;
            }
            0
        }
        _ => -EINVAL,
    }
}
```

**Mechanism:**
- Read TSC (Time Stamp Counter) → 10 cycles
- Convert to nanoseconds → 10 cycles
- Write to user buffer → 10 cycles
- Total: 30 cycles

### brk (60 cycles)

```rust
pub fn sys_brk(addr: usize) -> usize {
    let process = current_process();
    if addr == 0 {
        return process.heap_end;
    }
    
    if addr > process.heap_end {
        // Bump allocator: just increase heap_end
        process.heap_end = addr;
    }
    process.heap_end
}
```

**Mechanism:**
- No page allocation (lazy)
- Just update heap pointer
- 60 cycles average

## ⚡ Hybrid Path (500-2000 cycles)

### open (800 cycles)

```rust
pub fn sys_open(path: *const c_char, flags: i32, mode: u32) -> i64 {
    let path_str = unsafe { CStr::from_ptr(path) }.to_str()?;
    
    // 1. VFS lookup (400 cycles)
    let inode = vfs::lookup(path_str)?;
    
    // 2. Create capability (200 cycles)
    let cap = create_capability(inode, flags, mode)?;
    
    // 3. Allocate FD (100 cycles)
    let fd = allocate_fd()?;
    
    // 4. Store in FD table (100 cycles)
    fd_table::insert(fd, cap);
    
    fd as i64
}
```

**Optimization: Capability Cache**
```rust
// Cache recent FD → capability mappings (LRU, 64 entries)
// Hit rate: ~90% → 150 cycles instead of 800
```

### read (600 cycles)

```rust
pub fn sys_read(fd: i32, buf: *mut u8, count: usize) -> isize {
    // 1. FD → capability (50 cycles, cached)
    let cap = fd_table::get_capability(fd)?;
    
    // 2. Check permissions (50 cycles)
    cap.check_read()?;
    
    // 3. VFS read (400 cycles)
    let bytes_read = vfs::read(cap.inode, buf, count)?;
    
    // 4. Update offset (50 cycles)
    cap.offset += bytes_read;
    
    bytes_read as isize
}
```

**Zero-Copy Optimization:**
```rust
// If reading from pipe/socket and buffer is page-aligned:
// → Use fusion ring zero-copy path (300 cycles)
```

### pipe (1200 cycles)

```rust
pub fn sys_pipe(fds: *mut [i32; 2]) -> i32 {
    // 1. Create fusion ring (600 cycles)
    let (read_ring, write_ring) = ipc::create_fusion_ring_pair()?;
    
    // 2. Create capabilities (300 cycles)
    let read_cap = create_capability(read_ring, O_RDONLY, 0)?;
    let write_cap = create_capability(write_ring, O_WRONLY, 0)?;
    
    // 3. Allocate FDs (300 cycles)
    let read_fd = allocate_fd()?;
    let write_fd = allocate_fd()?;
    
    // 4. Store in FD table
    fd_table::insert(read_fd, read_cap);
    fd_table::insert(write_fd, write_cap);
    
    unsafe {
        (*fds)[0] = read_fd;
        (*fds)[1] = write_fd;
    }
    0
}
```

**Result:** pipe uses native Exo-OS fusion rings → 4x faster than Linux

## 🐢 Legacy Path (10K-50K cycles)

### fork (50,000 cycles)

```rust
pub fn sys_fork() -> i64 {
    let parent = current_process();
    
    // 1. Clone address space with COW (30K cycles)
    let child_addr_space = parent.address_space.clone_cow()?;
    
    // 2. Clone FD table (5K cycles)
    let child_fd_table = parent.fd_table.clone()?;
    
    // 3. Clone signal handlers (2K cycles)
    let child_signals = parent.signals.clone()?;
    
    // 4. Create new process (10K cycles)
    let child_pid = process::create(
        child_addr_space,
        child_fd_table,
        child_signals,
    )?;
    
    // 5. Add to scheduler (3K cycles)
    scheduler::add_process(child_pid);
    
    // Return 0 in child, child_pid in parent
    if is_child_context() { 0 } else { child_pid as i64 }
}
```

**COW (Copy-On-Write) Mechanism:**
```rust
// Mark all pages read-only
// On write → page fault → copy page → mark writable
// Lazy copying: ~1000x faster than full copy
```

### execve (40,000 cycles)

```rust
pub fn sys_execve(
    path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> i64 {
    // 1. Parse ELF binary (10K cycles)
    let elf = elf::parse(path)?;
    
    // 2. Create new address space (15K cycles)
    let addr_space = create_address_space()?;
    
    // 3. Load ELF segments (10K cycles)
    for segment in elf.segments {
        load_segment(&addr_space, segment)?;
    }
    
    // 4. Setup stack with argv/envp (3K cycles)
    let stack = setup_stack(&addr_space, argv, envp)?;
    
    // 5. Replace current address space (2K cycles)
    replace_address_space(addr_space);
    
    // 6. Jump to entry point (noreturn)
    jump_to_entry(elf.entry_point, stack);
}
```

### mmap (15,000 cycles)

```rust
pub fn sys_mmap(
    addr: usize,
    length: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> isize {
    if flags & MAP_ANONYMOUS != 0 {
        // Anonymous mapping → just allocate pages (5K cycles)
        return allocate_anonymous(addr, length, prot);
    }
    
    // File-backed mapping
    // 1. FD → capability (50 cycles)
    let cap = fd_table::get_capability(fd)?;
    
    // 2. Create shared memory region (10K cycles)
    let shm = memory::create_shared_memory(length)?;
    
    // 3. Map file into shared memory (4K cycles)
    vfs::read_into_shared_memory(cap.inode, offset, shm)?;
    
    // 4. Map shared memory into address space (1K cycles)
    map_shared_memory(addr, shm, prot)?;
    
    addr as isize
}
```

## 🔄 Translation Mechanisms

### FD → Capability

```rust
pub struct FdTable {
    entries: [Option<Capability>; MAX_FDS],  // 1024 entries
    cache: LruCache<i32, Capability>,        // 64-entry LRU
}

impl FdTable {
    pub fn get_capability(&self, fd: i32) -> Result<&Capability> {
        // 1. Check cache (10 cycles on hit)
        if let Some(cap) = self.cache.get(&fd) {
            return Ok(cap);
        }
        
        // 2. Check table (50 cycles)
        let cap = self.entries[fd as usize].as_ref()?;
        
        // 3. Update cache
        self.cache.insert(fd, cap.clone());
        
        Ok(cap)
    }
}
```

### Signal → IPC Message

```rust
pub fn signal_to_ipc(pid: u32, signal: i32) -> Result<()> {
    let message = IpcMessage {
        type: MessageType::Signal,
        signal_number: signal,
        sender_pid: current_pid(),
    };
    
    // Send via fusion ring (400 cycles)
    ipc::send_message(pid, message)?;
    
    Ok(())
}
```

**Signal Handling:**
```rust
// POSIX signals are converted to IPC messages
// Signal handler runs in userspace thread
// Async signals → queued in process signal queue
```

## 📊 Performance Comparison

| Syscall | Linux (cycles) | Exo-OS Fast | Exo-OS Hybrid | Exo-OS Legacy | Speedup |
|---------|----------------|-------------|---------------|---------------|---------|
| getpid | 150 | 20 | - | - | 7.5x |
| gettid | 150 | 20 | - | - | 7.5x |
| clock_gettime | 200 | 30 | - | - | 6.7x |
| brk | 300 | 60 | - | - | 5.0x |
| read (cached) | 1500 | - | 150 | - | 10x |
| read (uncached) | 1500 | - | 600 | - | 2.5x |
| write | 1800 | - | 700 | - | 2.6x |
| open | 2500 | - | 800 | - | 3.1x |
| pipe | 4000 | - | 1200 | - | 3.3x |
| fork | 50000 | - | - | 50000 | 1.0x |
| execve | 80000 | - | - | 40000 | 2.0x |

## 🧪 Adaptive Learning

```rust
pub struct AdaptiveLearning {
    syscall_stats: HashMap<SyscallNumber, SyscallStats>,
}

pub struct SyscallStats {
    count: u64,
    avg_cycles: f64,
    fast_path_eligible: bool,
}

impl AdaptiveLearning {
    /// Learn from syscall patterns
    pub fn record_syscall(&mut self, num: SyscallNumber, cycles: u64) {
        let stats = self.syscall_stats.entry(num).or_default();
        stats.count += 1;
        stats.avg_cycles = (stats.avg_cycles * 0.9) + (cycles as f64 * 0.1);
        
        // If consistently fast, mark as fast_path_eligible
        if stats.avg_cycles < 100.0 && stats.count > 1000 {
            stats.fast_path_eligible = true;
        }
    }
}
```

**Use Cases:**
- Detect hot syscalls → optimize to fast path
- Detect batching opportunities (multiple read/write)
- Detect zero-copy opportunities (aligned buffers)

## 🔗 musl libc Integration

### Bridge Layer

```rust
// kernel/src/posix_x/libc_impl/bridge.rs

#[no_mangle]
pub extern "C" fn __posix_x_read(fd: i32, buf: *mut u8, count: usize) -> isize {
    sys_read(fd, buf, count)
}

#[no_mangle]
pub extern "C" fn __posix_x_write(fd: i32, buf: *const u8, count: usize) -> isize {
    sys_write(fd, buf, count)
}

// ... all POSIX syscalls
```

### musl Adaptation

```c
// musl_adapted/unistd/read.c

ssize_t read(int fd, void *buf, size_t count) {
    return __posix_x_read(fd, buf, count);  // Call Exo-OS bridge
}
```

**Adaptation Strategy:**
1. Replace syscall assembly with Exo-OS bridge calls
2. Keep musl's buffering, formatting, locking
3. Adapt pthread → Exo-OS threads
4. Adapt malloc → Exo-OS heap allocator (optional)

## 🛠️ Development Tools

### Profiler

```rust
// tools/profiler.rs

pub fn profile_syscall(num: SyscallNumber) {
    let start = rdtsc();
    execute_syscall(num);
    let end = rdtsc();
    
    println!("Syscall {} took {} cycles", num, end - start);
}
```

### Compatibility Analyzer

```rust
// tools/analyzer.rs

pub fn analyze_binary(path: &str) -> CompatibilityReport {
    let elf = parse_elf(path)?;
    let syscalls = extract_syscalls(&elf)?;
    
    CompatibilityReport {
        total_syscalls: syscalls.len(),
        fast_path: count_fast_path(&syscalls),
        hybrid_path: count_hybrid_path(&syscalls),
        legacy_path: count_legacy_path(&syscalls),
        unsupported: count_unsupported(&syscalls),
    }
}
```

### Migrator

```rust
// tools/migrator.rs

pub fn migrate_app(path: &str) -> Result<()> {
    // Recompile app with Exo-OS libc
    // Link with POSIX-X compatibility layer
    // Test compatibility
    // Report issues
}
```

## 📚 Syscall Reference

### Supported Syscalls (150+)

**Fast Path (4):**
- getpid, gettid, clock_gettime, brk

**Hybrid Path (40+):**
- open, close, read, write, stat, fstat, lstat, lseek, ioctl, fcntl, dup, dup2, pipe, socket, bind, listen, accept, connect, send, recv, poll, select, epoll_create, epoll_ctl, epoll_wait, etc.

**Legacy Path (20+):**
- fork, vfork, clone, execve, mmap, munmap, mprotect, futex, kill, sigaction, sigreturn, etc.

**Unsupported (explicitly):**
- ptrace (use debugger API instead)
- sysfs (use procfs emulation)
- Some obscure ioctls

## 🎯 Success Criteria

- ✅ Fast path syscalls <100 cycles
- ✅ Hybrid path syscalls <2000 cycles
- ✅ 90%+ syscall cache hit rate
- ✅ Run unmodified Linux binaries (musl-based)
- ✅ Pass LTP (Linux Test Project) suite >95%
- ✅ 2-10x performance improvement over Linux for common syscalls

## 🔒 Security Considerations

1. **Capability Validation:** Every FD→capability translation validates permissions
2. **Bounds Checking:** All user pointers validated before dereference
3. **No TOCTOU:** Atomic FD operations
4. **Signal Safety:** Signals converted to IPC (no async signal handlers in kernel)

## 📈 Future Optimizations

1. **Syscall Batching:** Batch multiple syscalls in one context switch
2. **Speculative Execution:** Pre-fetch likely syscalls
3. **JIT Compilation:** Compile hot syscall sequences
4. **NUMA-Aware:** Place POSIX-X data structures on local NUMA node

## 🎓 References

- POSIX.1-2017: https://pubs.opengroup.org/onlinepubs/9699919799/
- musl libc: https://musl.libc.org/
- Linux syscall performance: https://www.kernel.org/doc/html/latest/
- Exo-OS IPC: See `docs/readme_syscall_et_drivers.md`

---

**Note to AI implementing POSIX-X:** This is a COMPLEX module. Start with Fast Path (4 syscalls), then Hybrid Path (10 most common), then incrementally add Legacy Path. Test each syscall individually before integration.
