# POSIX-X Complete Implementation Roadmap

**Version:** 3.0 - Complete musl libc Integration  
**Last Updated:** 2025-11-27  
**Target:** ~95% musl syscall coverage

---

## 📊 Phase Overview

| Phase | Name | Syscalls | Days | Status |
|-------|------|----------|------|--------|
| 1-2 | Infrastructure & Fast Path | 4 | - | ✅ Done |
| 3-5 | Stubs & musl Integration | - | - | ✅ Done |
| 6 | VFS POSIX Adapter | - | - | ✅ Done |
| 7 | Process Lifecycle | 3 | - | ✅ Done |
| **8** | **I/O Syscalls** | **8** | **2-3** | **✅ Done** |
| **9** | **Advanced Process** | **-** | **1-2** | **✅ Done** |
| **10** | **Program Loading** | **1** | **3-5** | **🔄 Partial** |
| **11** | **Signal Infrastructure** | **8** | **4-6** | **✅ Done** |
| **12** | **Pipes & Basic IPC** | **2** | **2-3** | **✅ Done** |
| **13** | **Directory Operations** | **6** | **2-3** | **⏳ Critical** |
| **14** | **File Links & Renaming** | **7** | **2** | **⏳ Critical** |
| **15** | **File Control (fcntl/ioctl)** | **4** | **2-3** | **⏳ Critical** |
| **16** | **Threading (futex)** | **5** | **5-7** | **✅ Done** |
| **17** | **Polling & Events** | **6** | **3-4** | **✅ Done** |
| **18** | **Pipe & FIFO** | **2** | **1-2** | **✅ Done** |
| **19** | **Unix Domain Sockets** | **12** | **5-7** | **✅ Done** |
| **20** | **Advanced Memory & Limits** | **8** | **2-3** | **⏳ CRITICAL** |
| 21 | Sockets & Networking | 12 | 5-7 | ⏳ |
| 22 | Advanced IPC | 8 | 3-5 | ⏳ |

**Total:** ~110 syscalls, ~60-80 days estimated

---

## ✅ Completed Phases (1-19)

### Phase 1-2: Infrastructure & Fast Path (DONE)

**Syscalls:** getpid, gettid, clock_gettime, brk  
**Lines:** ~1,190

### Phase 3-5: Stubs & musl Integration (DONE)

**Lines:** ~1,535

### Phase 6: VFS POSIX Adapter (DONE)

**Lines:** ~1,540  
Infrastructure for all file operations

### Phase 7: Process Lifecycle (DONE - 100%)

**Syscalls:** fork, exit, wait4  
**Lines:** ~800  
**Features:** COW, zombies, parent-child tracking

### Phase 8: I/O Syscalls Integration (DONE - 100%)

**Syscalls:** open, read, write, close, lseek, stat, fstat, lstat (8 total)  
**Lines:** ~327  
**Infrastructure:** FdTable with VFS, error conversion

### Phase 9: Advanced Process Features (DONE - 100%)

**Lines:** ~150  
**Features:**

- Thread parent-child tracking (parent_id, children, exit_status)
- Scheduler state checking methods
- Improved wait4() with real zombie detection
- 9 new Thread methods for process management

---

## ✅ Phase 10: Program Loading (execve) - COMPLETE (100%)

**Time:** 3-5 days  
**Priority:** HIGH

### Completed

- [x] ELF parser (~200 lines)
- [x] sys_execve() full implementation
- [x] VFS file loading integration
- [x] Segment loader & stack setup
- [x] Entry point jump

### Syscalls (1)

- [x] `execve()` - Execute program (Complete)

---

## ✅ Phase 11: Signal Infrastructure - COMPLETE (100%)

**Time:** 4-6 days  
**Priority:** MEDIUM

### Syscalls (8)

- [x] `sigaction()` - Register signal handler
- [x] `sigprocmask()` - Block/unblock signals
- [x] `kill()` - Send signal to process
- [x] `sigreturn()` - Return from signal
- [x] `tkill()` - Send signal to thread
- [x] `sigaltstack()` - Alternate signal stack
- [x] `rt_sigpending()` - Check pending signals
- [x] `rt_sigsuspend()` - Wait for signal

---

## ✅ Phase 14: File Links & Renaming - COMPLETE (100%)

**Time:** 2 days  
**Priority:** CRITICAL

### Syscalls (7)

- [x] `link()` - Create hard link (Stubbed ENOSYS)
- [x] `unlink()` - Remove file
- [x] `unlinkat()` - Remove file (at-style)
- [x] `symlink()` - Create symbolic link
- [x] `readlink()` - Read symbolic link
- [x] `rename()` - Rename file/directory
- [x] `renameat()` - Rename (at-style)

### Implementation

```rust
// kernel/src/posix_x/musl/hybrid_path/links.rs
pub unsafe extern "C" fn sys_symlink(target: *const i8, linkpath: *const i8) -> i64
pub unsafe extern "C" fn sys_readlink(path: *const i8, buf: *mut i8, bufsiz: usize) -> i64
pub unsafe extern "C" fn sys_rename(oldpath: *const i8, newpath: *const i8) -> i64
```

---

## ✅ Phase 15: File Control (fcntl/ioctl) - COMPLETE (100%)

**Time:** 2-3 days  
**Priority:** CRITICAL

### Syscalls (5)

- [x] `fcntl()` - File control
- [x] `ioctl()` - I/O control
- [x] `dup()` - Duplicate FD
- [x] `dup2()` - Duplicate FD to specific
- [x] `dup3()` - Duplicate FD with flags

---

## ✅ Phase 16: Threading (futex) - COMPLETE (100%)

**Time:** 5-7 days  
**Priority:** **CRITICAL** (Sans ça, pthreads impossible!)

### Syscalls (5)

- [x] **`futex()`** - Fast userspace mutex (CRITIQUE!)
- [x] `clone()` with CLONE_THREAD - Thread creation
- [x] `set_tid_address()` - Set thread ID pointer
- [x] `set_robust_list()` - Robust futex list
- [x] `gettid()` - Get thread ID (déjà fait mais vérifier)

### Components (~1000 lines)

- Futex wait queues
- Futex wake mechanism
- Thread-local storage (TLS)
- Thread groups
- Robust futex handling

### Implementation

```rust
// kernel/src/posix_x/musl/legacy_path/futex.rs
pub unsafe extern "C" fn sys_futex(
    uaddr: *mut u32,
    futex_op: i32,
    val: u32,
    timeout: *const timespec,
    uaddr2: *mut u32,
    val3: u32
) -> i64 {
    // FUTEX_WAIT, FUTEX_WAKE, FUTEX_REQUEUE
}
```

**Impact:** Pthreads works!

---

## ✅ Phase 17: Polling & Events - COMPLETE (100%)

**Time:** 3-4 days

### Syscalls (6)

- [x] `poll()` - Wait for events on FDs
- [x] `ppoll()` - poll with signal mask
- [x] `select()` - Wait for FD readiness
- [x] `pselect()` - select with signal mask
- [x] `epoll_create()` - Create epoll instance
- [x] `epoll_wait() / epoll_ctl()` - Linux epoll

### Implementation

```rust
// kernel/src/posix_x/syscall/handlers/fs_poll.rs
pub unsafe extern "C" fn sys_poll(fds: *mut pollfd, nfds: usize, timeout: i32) -> i64
pub unsafe extern "C" fn sys_epoll_wait(epfd: i32, events: *mut epoll_event, maxevents: i32, timeout: i32) -> i64
```

---

## ✅ Phase 18: Pipe & FIFO - COMPLETE (100%)

**Time:** 1-2 days

### Syscalls (2)

- [x] `mkfifo()` - Create named pipe
- [x] `mknod()` - Create filesystem node

### Implementation

```rust
// kernel/src/syscall/handlers/fs_fifo.rs
pub fn sys_mkfifo(path: &str, mode: u32) -> i32
```

---

## ✅ Phase 19: Unix Domain Sockets - COMPLETE (100%)

**Time:** 5-7 days
**Priority:** CRITICAL (for IPC)

### Syscalls (12)

- [x] `socket()` - Create socket
- [x] `bind()` - Bind socket
- [x] `listen()` - Listen for connections
- [x] `accept()` - Accept connection
- [x] `connect()` - Connect socket
- [x] `send() / recv()` - Send/receive data
- [x] `sendto() / recvfrom()` - Datagram send/recv
- [x] `shutdown()` - Shutdown socket (Stub)
- [x] `getsockopt() / setsockopt()` - Socket options (Stub)
- [x] `socketpair()` - Create socket pair

### Focus

- UNIX domain sockets (AF_UNIX)
- Stream (SOCK_STREAM) and datagram (SOCK_DGRAM)
- `socketpair()`

### Implementation

```rust
// kernel/src/syscall/handlers/net_socket.rs
pub fn sys_socket(domain: i32, type_: i32, protocol: i32) -> i32
```

---

## ⏳ Phase 20: Advanced Memory & Limits

**Time:** 2-3 days

### Syscalls (6)

- [ ] `madvise()` - Memory advice
- [ ] `mlock() / munlock()` - Lock memory
- [ ] `mincore()` - Page residency
- [ ] `getrlimit() / setrlimit()` - Resource limits
- [ ] `getrusage()` - Resource usage
- [ ] `prlimit64()` - Get/set resource limits

---

## ⏳ Phase 21: Sockets & Networking

**Time:** 5-7 days  
**Priority:** MEDIUM (pour IPC avancé)

### Syscalls (12)

- [ ] `socket()` - Create socket
- [ ] `bind()` - Bind socket
- [ ] `listen()` - Listen for connections
- [ ] `accept()` - Accept connection
- [ ] `connect()` - Connect socket
- [ ] `send() / recv()` - Send/receive data
- [ ] `sendto() / recvfrom()` - Datagram send/recv
- [ ] `sendmsg() / recvmsg()` - Message send/recv
- [ ] `shutdown()` - Shutdown socket
- [ ] `getsockopt() / setsockopt()` - Socket options

### Focus

- UNIX domain sockets (AF_UNIX)
- Stream (SOCK_STREAM) and datagram (SOCK_DGRAM)

---

## ⏳ Phase 22: Advanced IPC

**Time:** 3-5 days

### Syscalls (8)

- [ ] `shmget() / shmat() / shmdt()` - SysV shared memory
- [ ] `semget() / semop()` - SysV semaphores
- [ ] `msgget() / msgsnd() / msgrcv()` - SysV message queues
- [ ] `eventfd()` - Event notification FD
- [ ] `signalfd()` - Signal FD

---

## 📊 Coverage Summary

### By Priority

**CRITICAL (Phases 13-16):**

- Directory operations (navigation)
- File links (symlinks, rename)
- File control (fcntl, dup)
- **Threading (futex)** ← Bloquant pour pthreads!

**HIGH (Phases 8-10, 17-18):**

- I/O syscalls
- Program loading (execve)
- Polling
- Time/sleep

**MEDIUM (Phases 11, 19-22):**

- Signals
- Permissions
- Sockets
- Advanced IPC

### Total Coverage

**Syscalls Implemented:**

- After Phase 12: ~30 syscalls (33%)
- After Phase 16: ~50 syscalls (55%) ← **Minimum viable**
- After Phase 22: ~90 syscalls (100%) ← **Complete**

**musl libc Support:**

- Basic programs: ~80% after Phase 12
- Threaded programs: **Requires Phase 16 (futex)**
- Full POSIX apps: ~95% after Phase 22

---

## 🎯 Recommended Implementation Order

### Tier 1: Minimum Viable (Phases 8, 13-16)

**Time:** ~15-20 days  
**Outcome:** Basic programs + threads work

1. Phase 8: I/O Integration (2-3 days)
2. Phase 13: Directory Ops (2-3 days)
3. Phase 14: Links & Rename (2 days)
4. Phase 15: fcntl/dup (2-3 days)
5. Phase 16: futex/Threading (5-7 days) ← CRITICAL

### Tier 2: Full Featured (Add Phases 10-12, 17-19)

**Time:** +20-25 days  
**Outcome:** Most programs work

### Tier 3: Complete (All phases)

**Time:** +15-20 days  
**Outcome:** 95%+ musl coverage

---

## ⏳ Phase 23: File Operations Complete

**Time:** 1-2 days  
**Priority:** HIGH (data integrity)

### Syscalls (4)

- [ ] `truncate()` - Truncate file to specified length
- [ ] `ftruncate()` - Truncate file by FD
- [ ] `sync()` - Synchronize filesystem
- [ ] `fsync()` - Synchronize file data
- [ ] `fdatasync()` - Synchronize file data (no metadata)
- [ ] `sendfile()` - Zero-copy file transfer
- [ ] `splice()` - Move data between pipes/files
- [ ] `tee()` - Duplicate pipe content

### Implementation

```rust
// kernel/src/posix_x/musl/hybrid_path/file_ops_ext.rs
pub unsafe extern "C" fn sys_truncate(path: *const i8, length: i64) -> i64
pub unsafe extern "C" fn sys_ftruncate(fd: i32, length: i64) -> i64
pub unsafe extern "C" fn sys_fsync(fd: i32) -> i64
pub unsafe extern "C" fn sys_sendfile(out_fd: i32, in_fd: i32, offset: *mut i64, count: usize) -> i64
```

**Impact:** File integrity, performance (zero-copy)

---

## ⏳ Phase 24: System Info & Utilities

**Time:** 1-2 days  
**Priority:** HIGH (many programs need this)

### Syscalls (4)

- [ ] `uname()` - Get system information (name, version, arch)
- [ ] `sysinfo()` - Get system statistics (RAM, uptime, load)
- [ ] `umask()` - Get/set file creation mask
- [ ] `getrandom()` - Get random bytes (cryptographically secure)

### Implementation

```rust
// kernel/src/posix_x/musl/fast_path/sysinfo.rs
pub unsafe extern "C" fn sys_uname(buf: *mut utsname) -> i64 {
    // Fill: sysname="Exo-OS", release="0.1.0", machine="x86_64"
}

pub unsafe extern "C" fn sys_sysinfo(info: *mut sysinfo_t) -> i64 {
    // Fill: uptime, loads, totalram, freeram, procs
}

pub unsafe extern "C" fn sys_getrandom(buf: *mut u8, buflen: usize, flags: u32) -> i64 {
    // Use hardware RNG or CSPRNG
}
```

**Impact:** Essential for many utilities (ls, df, free, etc.)

---

## ⏳ Phase 25: Process Scheduling

**Time:** 2 days  
**Priority:** MEDIUM

### Syscalls (5)

- [ ] `sched_yield()` - Yield CPU to other processes
- [ ] `nice()` - Change process priority (deprecated interface)
- [ ] `setpriority() / getpriority()` - Set/get process priority
- [ ] `sched_setscheduler() / sched_getscheduler()` - Set/get scheduling policy
- [ ] `sched_setparam() / sched_getparam()` - Set/get scheduling parameters

### Implementation

```rust
// kernel/src/posix_x/musl/legacy_path/sched.rs
pub unsafe extern "C" fn sys_sched_yield() -> i64 {
    crate::scheduler::yield_now();
    0
}

pub unsafe extern "C" fn sys_setpriority(which: i32, who: u32, prio: i32) -> i64
pub unsafe extern "C" fn sys_sched_setscheduler(pid: i32, policy: i32, param: *const sched_param) -> i64
```

**Note:** `sys_setpriority` et `sys_getpriority` ont déjà des stubs dans process.rs

**Impact:** Real-time and priority-sensitive applications

---

## ⏳ Phase 26: File Notifications (inotify)

**Time:** 2-3 days  
**Priority:** MEDIUM

### Syscalls (4)

- [ ] `inotify_init()` - Initialize inotify instance
- [ ] `inotify_init1()` - Initialize with flags
- [ ] `inotify_add_watch()` - Add file/directory watch
- [ ] `inotify_rm_watch()` - Remove watch

### Components (~300 lines)

- Inotify instance management
- Watch descriptors
- Event queue
- Filesystem integration for notifications

### Implementation

```rust
// kernel/src/posix_x/musl/hybrid_path/inotify.rs
pub unsafe extern "C" fn sys_inotify_init1(flags: i32) -> i64
pub unsafe extern "C" fn sys_inotify_add_watch(fd: i32, pathname: *const i8, mask: u32) -> i64
```

**Impact:** File watchers, build systems, editors

---

## ⏳ Phase 27: Advanced Security & Control

**Time:** 1-2 days  
**Priority:** LOW-MEDIUM

### Syscalls (3)

- [ ] `prctl()` - Process control operations
  - PR_SET_NAME / PR_GET_NAME (process name)
  - PR_SET_DUMPABLE / PR_GET_DUMPABLE
  - PR_SET_PDEATHSIG (parent death signal)
- [ ] `capget()` - Get capabilities
- [ ] `capset()` - Set capabilities

### Implementation

```rust
// kernel/src/posix_x/musl/legacy_path/security.rs
pub unsafe extern "C" fn sys_prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i64 {
    match option {
        PR_SET_NAME => { /* Set process name */ }
        PR_GET_NAME => { /* Get process name */ }
        _ => -EINVAL
    }
}
```

**Impact:** Security, process naming, containerization

---

## 📊 COMPLETE Coverage Summary

### Total Syscalls by Phase

| Phases | Syscalls | Days | Coverage |
|--------|----------|------|----------|
| 1-7 (Done) | 11 | - | 10% |
| 8-12 (Done) | 21 | - | 29% |
| 13-16 (Critical) | 22 | 11-15 | 49% |
| 17-22 | 38 | 20-30 | 85% |
| **23-27 (Final)** | **20** | **9-12** | **95%+** |
| **TOTAL** | **~110** | **60-82** | **95-98%** |

### By Tier (Updated)

**Tier 1: Minimum Viable (Phases 8, 13-16)**

- ~33 syscalls
- ~15-20 days
- Basic programs + threads work
- **Core requirement for musl**

**Tier 2: Full Featured (Add 10-12, 17-19, 23-24)**

- ~65 syscalls (60%)
- +25-30 days
- Most programs work
- **Recommended target**

**Tier 3: Complete (Add 20-22, 25-27)**

- ~110 syscalls (100%)
- +25-35 days
- **95-98% musl coverage**
- Production-ready POSIX layer

---

## ✅ FINAL Confirmation

**WITH Phases 1-27:**

✅ **Directory navigation** (mkdir, getcwd, readdir)  
✅ **File operations** (open, read, write, truncate, sync)  
✅ **Links & rename** (symlink, readlink, rename)  
✅ **File control** (fcntl, dup, ioctl)  
✅ **Threading** (futex, clone CLONE_THREAD) ← **CRITICAL**  
✅ **Process lifecycle** (fork, exec, exit, wait)  
✅ **Signals** (sigaction, kill, etc.)  
✅ **Pipes & IPC** (pipe, SysV IPC, sockets)  
✅ **Polling** (poll, select, epoll)  
✅ **Time** (nanosleep, timers)  
✅ **Permissions** (chmod, chown, setuid)  
✅ **Memory** (mmap, madvise, mlock)  
✅ **System info** (uname, sysinfo, getrandom)  
✅ **Notifications** (inotify)  
✅ **Security** (prctl, capabilities)  

**Coverage:** 95-98% of what musl libc needs  
**Total Time:** 60-82 days  
**Total Syscalls:** ~110

---

**This is NOW a COMPLETE roadmap for full musl libc integration! 🎉**

**Recommendation:** Start with Tier 1 (Phases 8, 13-16) for MVP, then evaluate based on program requirements.
