# 📋 PLAN D'IMPLÉMENTATION USERSPACE - POUR VALIDATION

**Document:** Plan complet & optimisé pour création userspace Exo-OS  
**Audience:** Validation avant implémentation  
**Approche:** Sans stubs, TODOs, placeholders - Code REAL et FONCTIONNEL

---

## 🎯 PHASE 1 : TIMELINE COMPLET (20 jours)

### SEMAINE 1 : PRÉPARATION KERNEL (JOURS 1-7)

**Dépendances:** Bloquer toute création userspace tant que ces éléments ne sont pas COMPLETS

#### Jour 1-2: FdTable Implementation

**Travail kernel:**
```
✅ Objectif: Connecter fd table aux I/O syscalls

Fichiers:
  - kernel/src/process/mod.rs (FdTable structure)
  - kernel/src/process/fd_table.rs (NEW - FdTable impl)
  - kernel/src/arch/x86_64/syscall.rs (Connect sys_read/write/open/close)

APIs produced:
  sys_open(path: &str, flags: i32) → Fd
  sys_close(fd: Fd) → Result
  sys_read(fd: Fd, buf: &mut [u8]) → usize
  sys_write(fd: Fd, buf: &[u8]) → usize
  
Pre-test: Can open /bin/hello, write to stdout
```

**Validation:** 
```
Test: exo-os write_test
Expected: "Test write" printed to console
```

---

#### Jour 3-5: exec() VFS Loading (JOUR 4-5 planifié)

**Travail kernel:**
```
✅ Objectif: Load real ELF binaries from VFS

Fichiers:
  - kernel/src/loader/elf.rs (Parser ELF - exists, enhance)
  - kernel/src/loader/spawn.rs (load_elf_from_vfs - NEW)
  - kernel/src/syscall/handlers/process.rs (sys_execve - enhance)

APIs produced:
  load_elf_from_vfs(path: &str) → Result<Entry>
  exec(path: &str, args: &[&str]) → !

Features:
  - Open file from VFS
  - Read ELF header + program headers
  - Map PT_LOAD segments to memory
  - Setup stack with argc/argv/envp
  - Transfer execution
```

**Validation:**
```
Test: exo-os exec /bin/hello
Expected: 
  - File opened via VFS
  - ELF parsed correctly
  - "Hello from Exo-OS!" printed
  - Process terminates with exit(0)
```

---

#### Jour 6: Address Space Reconstruction

**Travail kernel:**
```
✅ Objectif: Complete fork() address space cloning

Fichiers:
  - kernel/src/memory/virtual_mem/address_space.rs (Fix)
  - kernel/src/scheduler/thread/thread.rs (Ensure address_space field)

Fixes:
  - Uncomment CoW manager initialization
  - Implement address space reconstruction in fork()
  - Add address_space to thread clone
  - Verify CoW on page fault

Post-condition: fork() creates independent address spaces
```

**Validation:**
```
Test: Test fork() with memory modifications
Expected:
  - Parent and child have independent memory
  - CoW triggered on write
  - No cross-pollution
```

---

#### Jour 7: Integration Test

**Travail kernel + simple test:**
```
✅ Objective: Verify all kernel pieces work together

Test program: userland/test_kernel_ready.c
  - #include <unistd.h>
  - int main() { write(1, "Ready\n", 6); return 0; }

Compile via musl-gcc

Test procedure:
  1. Boot kernel
  2. shell$ exec /bin/test_kernel_ready
  3. Expect: "Ready" printed, process exits

If fails: Debug step-by-step
  - Can we open file?
  - Can we parse ELF?
  - Can we read stdout?
```

**Gate:** ✅ **Phase 1a is BLOCKED until this passes!**

---

### SEMAINE 2 : SERVICES (JOURS 8-14)

#### Jour 8-9: init Service

**Work required: ~400 LOC**

```
File: userland/init/src/main.rs (CREATE NEW)

Structure:
  1. VFS mount setup
     - tmpfs on /tmp
     - Auto-mounts by kernel: devfs, procfs, sysfs
     
  2. Signal handling
     - Register SIGCHLD handler
     - Reap zombie children
     
  3. Service spawning
     - fork() + exec() for each service
     - fs_service first (required for mounting)
     - shell second (user interface)
     
  4. Main loop
     - Wait for SIGCHLD
     - Reap dead children
     - Check service health

Dependencies:
  - exo_std::process (fork, exec, wait4)
  - exo_std::fs (mount)
  - exo_std::io (write)

Compile: In userland/init/, cargo build --release
Result: userland/init/target/release/init (copy to /init in initramfs)
```

**Testing:**
```
Boot sequence validation:
  1. Kernel exits to shell
  2. shell$ exec /init
  3. init runs, mounts /tmp
  4. init spawns fs_service
  5. init spawns shell
  6. shell prompt appears
  
Expected output:
  [INIT] VFS setup OK
  [INIT] fs_service spawned (PID=2)
  [INIT] Shell spawned (PID=3)
  [INIT] Init process ready
  # (shell prompt)
```

**Validation gate:** ✅ init starts and spawns children

---

#### Jour 10-11: fs_service Daemon

**Work required: ~200 LOC (Phase 1 minimal version)**

```
File: userland/fs_service/src/main.rs (CREATE NEW)

Phase 1 design:
  - Minimal daemon (keeps process alive)
  - mount()/umount() syscalls handled by kernel
  - fs_service just registers as service
  
Phase 2 design (deferred):
  - IPC-based mount management
  - Filesystem hot-plugging
  - Mount namespace support

Current code:
  fn main() {
      println!("[FS_SERVICE] Started");
      
      // Register with service manager (Phase 2)
      
      // Main loop
      loop {
          pause(); // Wait for signals
          // Handle mount requests (Phase 2)
      }
  }

Compile: In userland/fs_service/, cargo build --release
Result: fs_service binary in target/release/
```

**Testing:**
```
Boot verification:
  1. init spawns fs_service
  2. fs_service prints start message
  3. fs_service stays running
  
Expected in init output:
  [INIT] fs_service spawned (PID=2)
  [FS_SERVICE] Started
  # (init continues)
```

**Validation gate:** ✅ fs_service spawns and stays alive

---

#### Jour 12-13: Shell Completion

**Work required: +200 LOC (additions to existing 1222 LOC)**

```
File: userland/shell/src/builtin.rs (ENHANCE)

Add missing commands:
  ✅ EXISTING (14): ls, cat, mkdir, touch, write, rm, pwd, cd, help, exit, version, clear, stat, mount
  🔴 ADD (8): cp, mv, echo, ps, kill, chmod, grep, find

Quick implementations:

// cp - copy file
fn cmd_cp(args: &[&str]) -> Result<()> {
    let src = fs::read(args[0])?;
    fs::write(args[1], src)?;
    Ok(())
}

// echo - print arguments
fn cmd_echo(args: &[&str]) -> Result<()> {
    for arg in args { print!("{} ", arg); }
    println!();
    Ok(())
}

// ps - list processes
fn cmd_ps() -> Result<()> {
    let pids = fs::read_dir("/proc")?;
    for entry in pids {
        if let Ok(stat) = fs::read_to_string(&format!("/proc/{}/stat", entry)) {
            println!("{}", stat);
        }
    }
    Ok(())
}

// kill - send signal to process  
fn cmd_kill(args: &[&str]) -> Result<()> {
    let pid: i32 = args[0].parse()?;
    signal::kill(pid, signal::SIGTERM)?;
    Ok(())
}

Compile: cargo build --release in userland/shell/
```

**Testing:**
```
shell$ help
shell$ echo hello world
shell$ mkdir /tmp/test
shell$ cp file1 /tmp/test/file2
shell$ ps
shell$ kill [pid]
shell$ mount tmpfs /mnt tmpfs

Expect: All commands work with proper output
```

**Validation gate:** ✅ All shell commands work

---

### SEMAINE 3 : TEST BINARIES (JOURS 14-20)

#### Jour 14-15: Basic Test Binaries (test_hello, test_args)

```
File: userland/test_hello.c (UPDATE)
File: userland/test_args.c (CREATE)

test_hello.c:
  #include <unistd.h>
  int main() {
      write(1, "Hello from Exo-OS!\n", 19);
      _exit(0);
  }

test_args.c:  
  #include <stdio.h>
  int main(int argc, char **argv) {
      for (int i = 0; i < argc; i++) {
          printf("argv[%d] = %s\n", i, argv[i]);
      }
      return 0;
  }

Compile:
  musl-gcc -static test_hello.c -o test_hello
  musl-gcc -static test_args.c -o test_args

Test:
  shell$ /test_hello
  Hello from Exo-OS!
  
  shell$ /test_args abc def ghi
  argv[0] = /test_args
  argv[1] = abc
  argv[2] = def
  argv[3] = ghi
```

**Validation gates:**
- ✅ Processes load and execute
- ✅ Argument passing works
- ✅ Output appears on console

---

#### Jour 16-17: Process Tests (test_fork, test_pipe)

```
File: userland/test_fork.c (CREATE/UPDATE)
File: userland/test_pipe.c (CREATE/UPDATE)

test_fork.c:
  Tests fork() system call
  Parent spawns child
  Child modifies memory (independent)
  Child exits
  Parent waits and gets exit code
  
test_pipe.c:
  Tests pipe() for IPC
  Parent creates pipe
  Writes message
  Reads back
  Verifies content

Compile:
  musl-gcc -static test_fork.c -o test_fork
  musl-gcc -static test_pipe.c -o test_pipe

Test:
  shell$ /test_fork
  [Parent] Forking...
  [Child PID=5] Running
  [Parent] Child exited with code 42

  shell$ /test_pipe
  Writing to pipe...
  Reading from pipe...
  Received: Hello from pipe!
```

**Validation gates:**
- ✅ fork() works (PID allocation)
- ✅ wait4() retrieves exit code
- ✅ pipe() IPC works
- ✅ read/write on pipes

---

#### Jour 18-19: Signal & Mount Tests (test_signals, test_mount)

```
File: userland/test_signals.c (CREATE)
File: userland/test_mount.c (CREATE)

test_signals.c:
  Register signal handler for SIGTERM
  Print PID
  pause()
  User sends kill -TERM from another shell
  Handler prints "Received SIGTERM"
  
test_mount.c:
  mkdir /mnt/test
  mount("tmpfs", "/mnt/test", "tmpfs")
  create file in /mnt/test
  umount("/mnt/test")
  Verify file no longer accessible

Compile:
  musl-gcc -static test_signals.c -o test_signals  
  musl-gcc -static test_mount.c -o test_mount

Test (multi-shell):
  # Shell 1
  shell$ /test_signals
  Process ready. Send SIGTERM to PID 7
  
  # Shell 2
  shell$ kill -TERM 7
  
  # Shell 1
  Received SIGTERM
  
  # Single shell
  shell$ /test_mount
  Mounting tmpfs on /mnt/test...
  Mount successful!
  Created /mnt/test/file.txt
  Unmounting /mnt/test...
  Unmount successful!
```

**Validation gates:**
- ✅ Signal delivery works
- ✅ Handlers are invoked
- ✅ mount() system call works
- ✅ umount() system call works
- ✅ Filesystem isolation works

---

#### Jour 20: FULL INTEGRATION TEST

**Complete boot sequence test:**

```
Procedure:
  1. kernel boots
  2. Kernel hands off to /init
  3. init mounts /tmp
  4. init spawns fs_service
  5. init spawns shell
  6. Shell appears with prompt
  
  7. Test each core feature:
     shell$ /test_hello          → logs hello
     shell$ /test_args 1 2 3     → shows arguments
     shell$ /test_fork           → fork + wait
     shell$ /test_pipe           → pipe IPC
     shell$ /test_mount          → mount/umount
     shell$ /test_signals [pid]  → signal delivery
     
  8. Test shell features:
     shell$ ls /bin
     shell$ mkdir /tmp/foo
     shell$ cp /test_hello /tmp/foo/
     shell$ ps
     shell$ mount
     shell$ echo "test"
     
  9. Advanced:
     shell$ /test_hello > /tmp/output.txt
     shell$ cat /tmp/output.txt
     → "Hello from Exo-OS!"

Expected result: All tests PASS
```

**Validation gate:** ✅ **PHASE 1 COMPLETE AND WORKING**

---

## 📊 PHASE 2+ ROADMAP (NOT PHASE 1)

**Only start after Phase 1 is COMPLETE**

### Phase 2a: Networking (Week 4-5)
- Socket API (AF_INET, SOCK_STREAM/DGRAM)
- TCP/UDP basic implementation
- Test: simple ping, echo server

### Phase 2b: Drivers (Week 5-6)
- PCI enumeration
- Network driver (VirtIO, E1000)
- Block device (FAT32 mounting)

### Phase 3: Advanced (Month 2+)
- UI/Wayland (DEFERRED - not critical)
- AI modules (DEFERRED - nice-to-have)
- Complex filesystems (ext4)

---

## ✅ CRITICAL RESTRICTIONS FOR THIS PLAN

### GOLDEN RULES

1. **NO STUBS** - Every function must have real implementation
2. **NO TODOs** - Comment TODOs only in kernel (pre-Phase1), never in userspace code
3. **NO PLACEHOLDERS** - Remove unused files/functions
4. **CODE QUALITY** - All functions documented with purpose, parameters, return values
5. **ERROR HANDLING** - Every syscall checked for errors
6. **TESTING** - Each feature tested before moving to next

### FORBIDDEN

❌ Empty main.rs files (remove or implement)  
❌ TODO comments in userspace  
❌ unimplemented!() macros  
❌ unused imports  
❌ unsafe without documentation  
❌ Skipping Phase 1 steps to rush Phase 2

### MANDATORY

✅ All test binaries must execute successfully  
✅ All shell commands must work  
✅ Clean compile (cargo build --release)  
✅ No panics during normal operation  
✅ All syscalls properly connected  
✅ Code walkthrough before merging (self-review)

---

## 🚀 VALIDATION POINTS (GO/NO-GO GATES)

| Day | Gate | Requirement |
|-----|------|------------|
| **7** | Phase 1a Ready | exec /bin/test_kernel_ready works |
| **9** | init Ready | init spawns fs_service, shell |
| **11** | fs_service Ready | fs_service stays alive |
| **13** | shell Ready | All shell commands work |
| **15** | Tests Ready | test_hello + test_args execute |
| **17** | Process Ready | fork/pipe work correctly |
| **19** | Advanced Ready | signals/mount work |
| **20** | PHASE 1 COMPLETE | All integration tests PASS ✓ |

---

## 📝 DELIVERABLES AT END OF PHASE 1

### Kernel
```
✅ Complete syscalls: open, close, read, write, exec, fork, wait4, signal
✅ Working VFS: tmpfs, devfs, procfs, sysfs
✅ Real ELF loading from VFS
✅ Address space cloning + CoW
✅ Signal delivery
✅ No stubs in critical path
```

### Userspace
```
✅ /init (service starter)
✅ /fs_service (filesystem daemon)
✅ /shell (14+ commands)
✅ /bin/test_hello (hello world)
✅ /bin/test_args (argument passing)
✅ /bin/test_fork (process creation)
✅ /bin/test_pipe (IPC)
✅ /bin/test_signals (signal handling)
✅ /bin/test_mount (filesystem mounting)

Codeliness: 
  - init: 400 LOC
  - fs_service: 200 LOC  
  - shell +200 LOC
  - test binaries: 400 total LOC
  - TOTAL NEW: ~1200 LOC
```

### Documentation
```
✅ This plan (complete)
✅ Audit diagnostic (complete)
✅ Code comments (in all new files)
✅ Commit messages (clear, descriptive)
✅ Test results (logged)
```

---

## 🎯 FINAL NOTE

**Objective:** Transform Exo-OS from 95% single-threaded kernel with empty userspace to a **real, working operating system** where:
- Users can execute programs
- Programs can interact with filesystem
- Programs can fork/exec/IPC
- All core POSIX features work

**Approach:** Methodical, step-by-step, NO shortcuts or simplifications

**Timeline:** 20 days (3 weeks) for Phase 1  
**Quality:** Enterprise-grade code, fully featured, properly tested

---

**READY FOR VALIDATION? ✓**

