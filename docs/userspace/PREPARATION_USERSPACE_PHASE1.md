# 🚀 PRÉPARATION USERSPACE - PHASE 1 (FINAL)

**Objectif:** Créer un userspace RÉEL et FONCTIONNEL sans stubs, TODOs, placeholders  
**Date:** 4 février 2026  
**Durée estimée:** 2-3 semaines (Phase 1 complète)  
**Dépendances kernel:** FdTable + exec() VFS + address space complet

---

## 📋 VUE D'ENSEMBLE PHASE 1

### Services à créer (Ordre de dépendance)

| # | Service | Dépendances | Priorité | LOC Estimé | Libs requises |
|---|---------|-------------|----------|---------|--------------|
| **1** | init | kernel seulement | 🔴 CRITIQUE | 300-400 | exo_std |
| **2** | fs_service | init, kernel VFS | 🔴 CRITIQUE | 500-700 | exo_std, fs APIs |
| **3** | Test binaries | init, fs_service, exec() | 🔴 CRITIQUE | 200-300 | exo_std, musl |
| **4** | shell (compléter) | init, fs_service | 🟡 IMPORTANT | +200 | exo_std |
| **5** | services framework | init | 🟡 IMPORTANT | 300-400 | exo_std, IPC |
| **6** | net_service | shell (optionnel) | 🟢 FUTURE | 800-1200 | exo_std, net APIs |

### Binaires à créer (Ordre de dépendance)

| # | Binaire | Type | Test | Dépend de |
|---|---------|------|------|-----------|
| **1** | test_hello | Simple I/O | write() | exec() |
| **2** | test_args | Process | argv/envp | exec() + args |
| **3** | test_fork | Process | fork/wait | fork() |
| **4** | test_pipe | IPC | pipe I/O | pipe() |
| **5** | test_signals | Signals | signal handlers | signal() |
| **6** | test_mount | VFS | mount/umount | fs_service |

---

## ⚙️ SECTION 1 : SERVICE INIT

### Objectif
- Premier processus lancé après kernel
- Initialise VFS mount points
- Lance fs_service
- Lance shell / services
- Gère signal SIGCHLD (reap zombies)

### Architecture

```
kernel_main()
    ↓
execute /init (via exec() syscall)
    ↓
init::main()
    ├── Setup VFS mounts (tmpfs, devfs, procfs)
    ├── Spawn fs_service (fork/exec)
    ├── Wait for fs_service ready
    ├── Mount real filesystems (if available)
    ├── Spawn shell or other services
    └── Main loop: handle SIGCHLD
```

### Implémentation: init/src/main.rs

**Fichier à créer:** `userland/init/src/main.rs`

```rust
#![no_std]
#![no_main]

use exo_std::prelude::*;
use exo_std::process::{fork, exec, wait4, exit};
use exo_std::fs::{mount, umount};
use exo_std::signal::{signal, SIGCHLD};

// Signal handler for SIGCHLD
static mut SIGCHLD_RECEIVED: bool = false;

extern "C" fn sigchld_handler(_sig: i32) {
    unsafe { SIGCHLD_RECEIVED = true; }
}

#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    // Step 1: Setup VFS mounts
    if let Err(e) = setup_vfs() {
        eprintln!("[INIT] VFS setup failed: {:?}", e);
        return 1;
    }
    write_stdout("[INIT] VFS setup OK\n");

    // Step 2: Register SIGCHLD handler
    unsafe {
        signal(SIGCHLD, sigchld_handler as *const ());
    }

    // Step 3: Spawn fs_service
    let fs_pid = match spawn_service("fs_service", &[]) {
        Ok(pid) => {
            write_stdout("[INIT] fs_service spawned (PID=");
            write_stdout_int(pid);
            write_stdout(")\n");
            pid
        }
        Err(e) => {
            eprintln!("[INIT] Failed to spawn fs_service: {:?}", e);
            return 1;
        }
    };

    // Step 4: Spawn shell
    let shell_pid = match spawn_service("shell", &[]) {
        Ok(pid) => {
            write_stdout("[INIT] Shell spawned (PID=");
            write_stdout_int(pid);
            write_stdout(")\n");
            pid
        }
        Err(e) => {
            eprintln!("[INIT] Failed to spawn shell: {:?}", e);
            return 1;
        }
    };

    // Step 5: Main loop - wait for children
    write_stdout("[INIT] Init process ready. Waiting for children...\n");
    
    loop {
        unsafe {
            if SIGCHLD_RECEIVED {
                SIGCHLD_RECEIVED = false;
                // Reap zombies
                loop {
                    match wait4(-1, null_mut(), 0, null_mut()) {
                        Ok(wpid) if wpid == 0 => break,
                        Ok(wpid) => {
                            write_stdout("[INIT] Child ");
                            write_stdout_int(wpid);
                            write_stdout(" exited\n");
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        
        // Check if critical services are down
        if fs_pid > 0 && shell_pid > 0 {
            // Keep waiting
            pause(); // Wait for signals
        } else {
            break;
        }
    }

    write_stdout("[INIT] Init exiting\n");
    exit(0)
}

fn setup_vfs() -> Result<(), Box<dyn std::error::Error>> {
    // Mount tmpfs on /tmp
    mount("tmpfs", "/tmp", "tmpfs", 0, "")?;
    
    // Note: devfs, procfs, sysfs should auto-mount by kernel
    // But can be done here if needed
    
    Ok(())
}

fn spawn_service(name: &str, args: &[&str]) -> Result<i32> {
    let pid = fork()?;
    
    if pid == 0 {
        // Child process
        let mut argv = vec![name.as_ptr() as *const u8];
        for arg in args {
            argv.push(arg.as_ptr() as *const u8);
        }
        argv.push(null());
        
        // Look for binary in /bin or /sbin
        let binary_path = format!("/bin/{}", name);
        exec(&binary_path, argv.as_ptr())?;
        
        // If exec fails
        eprintln!("[INIT] exec failed for {}", name);
        exit(1);
    } else {
        // Parent
        Ok(pid)
    }
}

// Helper functions for no_std environment
fn write_stdout(s: &str) {
    let _ = exo_std::io::write(1, s.as_bytes());
}

fn write_stdout_int(n: i32) {
    let s = alloc::format!("{}", n);
    write_stdout(&s);
}

fn eprintln(s: &str) {
    let _ = exo_std::io::write(2, format!("[ERROR] {}\n", s).as_bytes());
}

#[panic_handler]
fn panic(_pi: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

**Dépendances:**
- `exo_std::process` (fork, exec, wait4, exit)
- `exo_std::fs` (mount, umount)  
- `exo_std::signal` (signal handler registration)
- `exo_std::io` (write)

---

## ⚙️ SECTION 2 : SERVICE FS_SERVICE

### Objectif
- Wrapper VFS du kernel
- Gère mounting/unmounting
- Expose filesystem via IPC (futur)
- Pour Phase 1: simple mount/umount daemon

### Architecture

```
Application
    ↓ syscall mount()
VFS kernel (fs_service routes through)
    ├── FAT32
    ├── ext4
    └── tmpfs/devfs/procfs
```

### Implémentation: fs_service/src/main.rs

**Fichier à créer:** `userland/fs_service/src/main.rs`

```rust
#![no_std]
#![no_main]

use exo_std::prelude::*;
use exo_std::fs::*;
use exo_std::io::write;
use exo_std::process::exit;

#[no_mangle]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    write_stdout("[FS_SERVICE] Initialized\n");
    
    // For Phase 1: fs_service is minimal
    // Just keeps running, handler mount() syscalls via kernel
    
    // In future: IPC-based mount management
    
    // Main loop
    loop {
        // Wait for signals or commands
        // For now, just sleep
        crate::syscall::pause();
    }
}

fn write_stdout(s: &str) {
    let _ = write(1, s.as_bytes());
}

#[panic_handler]
fn panic(_pi: &core::panic::PanicInfo) -> ! {
    loop {}
}
```

**Phase 1:** Minimal. Mount syscalls vont directement au kernel.  
**Phase 2:** Ajouter IPC communication pour mount management.

---

## ⚙️ SECTION 3 : TEST BINARIES

### 3.1 test_hello

**Fichier:** `userland/test_hello.c`

```c
#include <unistd.h>

int main(void) {
    write(1, "Hello from Exo-OS!\n", 19);
    _exit(0);
}
```

**Compile:** `musl-gcc -static test_hello.c -o test_hello`

**Test:** `exo-os$ /test_hello`  
**Résultat attendu:** `Hello from Exo-OS!`

**Valide:**
- ✅ exec() loading ELF
- ✅ write() syscall
- ✅ exit() syscall

---

### 3.2 test_args

**Fichier:** `userland/test_args.c`

```c
#include <unistd.h>
#include <stdio.h>

int main(int argc, char **argv) {
    printf("Arguments: %d\n", argc);
    for (int i = 0; i < argc; i++) {
        printf("  argv[%d] = %s\n", i, argv[i]);
    }
    return 0;
}
```

**Test:** `exo-os$ /test_args hello world`  
**Résultat attendu:**
```
Arguments: 3
  argv[0] = /test_args
  argv[1] = hello
  argv[2] = world
```

**Valide:**
- ✅ argc/argv passing
- ✅ Argument parsing
- ✅ String handling

---

### 3.3 test_fork

**Fichier:** `userland/test_fork.c`

```c
#include <unistd.h>
#include <stdlib.h>
#include <stdio.h>
#include <sys/wait.h>

int main(void) {
    pid_t pid = fork();
    
    if (pid == 0) {
        // Child
        printf("Child process (PID=%d)\n", getpid());
        exit(42);
    } else if (pid > 0) {
        // Parent
        int status = 0;
        waitpid(pid, &status, 0);
        printf("Parent: child exited with ";
        printf("status %d\n", WEXITSTATUS(status));
        exit(0);
    } else {
        perror("fork failed");
        exit(1);
    }
}
```

**Test:** `exo-os$ /test_fork`  
**Résultat attendu:**
```
Child process (PID=2)
Parent: child exited with status 42
```

**Valide:**
- ✅ fork() system call
- ✅ PID management
- ✅ wait4() / waitpid()
- ✅ Exit status

---

### 3.4 test_pipe

**Fichier:** `userland/test_pipe.c`

```c
#include <unistd.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    int pipefd[2];
    char buf[100];
    
    if (pipe(pipefd) == -1) {
        perror("pipe");
        return 1;
    }
    
    printf("Writing to pipe...\n");
    const char *msg = "Hello from pipe!";
    write(pipefd[1], msg, strlen(msg));
    close(pipefd[1]);
    
    printf("Reading from pipe...\n");
    ssize_t n = read(pipefd[0], buf, sizeof(buf) - 1);
    if (n > 0) {
        buf[n] = '\0';
        printf("Received: %s\n", buf);
    }
    close(pipefd[0]);
    
    return 0;
}
```

**Test:** `exo-os$ /test_pipe`  
**Résultat attendu:**
```
Writing to pipe...
Reading from pipe...
Received: Hello from pipe!
```

**Valide:**
- ✅ pipe() syscall
- ✅ read() / write() on pipes
- ✅ File descriptor handling

---

### 3.5 test_signals

**Fichier:** `userland/test_signals.c`

```c
#include <unistd.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>

static volatile int signal_received = 0;

void signal_handler(int sig) {
    signal_received = sig;
}

int main(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = signal_handler;
    
    if (sigaction(SIGTERM, &sa, NULL) < 0) {
        perror("sigaction");
        return 1;
    }
    
    printf("Process ready. Send SIGTERM to PID %d\n", getpid());
    
    // Wait for signal
    pause();
    
    if (signal_received == SIGTERM) {
        printf("Received SIGTERM\n");
    }
    
    return 0;
}
```

**Test (dans deux shells):**
```bash
# Shell 1
exo-os$ /test_signals
Process ready. Send SIGTERM to PID 5

# Shell 2
exo-os$ kill -TERM 5

# Shell 1
Received SIGTERM
```

**Valide:**
- ✅ sigaction() registration
- ✅ Signal delivery
- ✅ Signal handlers

---

### 3.6 test_mount

**Fichier:** `userland/test_mount.c`

```c
#include <unistd.h>
#include <stdio.h>
#include <sys/mount.h>
#include <sys/stat.h>

int main(void) {
    // Create mount point
    mkdir("/mnt", 0755);
    
    printf("Mounting tmpfs on /mnt...\n");
    if (mount("tmpfs", "/mnt", "tmpfs", 0, "") < 0) {
        perror("mount failed");
        return 1;
    }
    
    printf("Mount successful!\n");
    
    // Test: create file in mounted tmpfs
    FILE *f = fopen("/mnt/test.txt", "w");
    if (f) {
        fprintf(f, "Hello from tmpfs\n");
        fclose(f);
        printf("Created /mnt/test.txt\n");
    }
    
    // Unmount
    printf("Unmounting /mnt...\n");
    if (umount("/mnt") < 0) {
        perror("umount failed");
        return 1;
    }
    
    printf("Unmount successful!\n");
    return 0;
}
```

**Test:** `exo-os$ /test_mount`  
**Résultat attendu:**
```
Mounting tmpfs on /mnt...
Mount successful!
Created /mnt/test.txt
Unmounting /mnt...
Unmount successful!
```

**Valide:**
- ✅ mount() syscall
- ✅ umount() syscall
- ✅ Directory operations
- ✅ File I/O in mounted filesystem

---

## 📦 SECTION 4: SHELLCOMPLETION

### État actuel
- ✅ 1222 LOC de code
- ✅ 14 builtin commands  
- ✅ Parser complet
- 🟡 AI integration stub (4 LOC)

### À compléter

**Commandes manquantes (Priorité haute):**

```
✅ ls, cat, mkdir, touch, write, rm
✅ pwd, cd, help, exit, version
🔴 cp (copy file)
🔴 mv (move file)
🔴 chmod (permissions)
🔴 mount/umount (filesystem)
🔴 ps (list processes)
🔴 kill (send signals)
🔴 grep (search)
🔴 echo (print)
```

**Implémentations rapides à ajouter:**

```rust
// shell/src/builtin.rs - Add these

fn cmd_cp(args: &[&str]) -> Result<()> {
    if args.len() < 2 {
        println!("Usage: cp <src> <dst>");
        return Ok(());
    }
    fs_copy_file(args[0], args[1])?;
    Ok(())
}

fn cmd_echo(args: &[&str]) -> Result<()> {
    for arg in args {
        print!("{} ", arg);
    }
    println!();
    Ok(())
}

fn cmd_ps() -> Result<()> {
    // List processes from /proc
    let entries = os_read_dir("/proc")?;
    println!("PID   NAME");
    for entry in entries {
        // Parse PID from /proc/[pid]/stat
        let pid = entry.name.parse::<i32>()?;
        let stat = fs_read_to_string(&format!("/proc/{}/stat", pid))?;
        println!("{:<5} {}", pid, stat);
    }
    Ok(())
}
```

---

## 📋 SECTION 5 : DÉPENDANCES & LIBS

### exo_std library (Wrapper Rust autour kernel APIs)

**Modules requis:**

```
exo_std::
├── prelude        # Essential imports
├── process        # fork, exec, wait4, exit, getpid
├── fs             # open, read, write, close, mkdir, mount
├── signal         # signal, kill, sigaction
├── io             # read, write (low-level)
├── ipc            # (future) fusionrings, sockets
└── memory         # (future) mmap, mprotect
```

**État:** Partially existant, à compléter selon besoins

### musl libc (Externe)

**État:** ✅ 150K+ LOC, compilé, fonctionnel

**Usage:** `musl-gcc -static app.c -o app`

---

## 🎯 SECTION 6 : PLAN D'IMPLÉMENTATION (TIMELINE)

### Phase 1a: Préparation Kernel (1 semaine) 
**Préalable avant tout userspace!**

**Jours 1-2: FdTable**
- [ ] Implémenter FdTable simple dans kernel
- [ ] Connecter sys_read/write/open/close
- [ ] Tests kernel fd operations

**Jours 3-5: exec() VFS loading** (TODO Jour 4-5 planifié)
- [ ] load_elf_from_vfs()
- [ ] PT_LOAD mapping
- [ ] argv/envp stack setup
- [ ] Dynamic linker support (Optional)

**Jour 6: Address space fixes**
- [ ] Finir address space reconstruction
- [ ] CoW finalization
- [ ] Test fork() complet

**Jour 7: Integration test**
- [ ] Test /bin/hello loading via exec()
- [ ] Test argv passing
- [ ] Test write() output

---

### Phase 1b: Init + Services (1 semaine)

**Jour 8-9: init service**
- [ ] Créer init/src/main.rs (300-400 LOC)
- [ ] VFS setup (mount tmpfs, etc.)
- [ ] Service spawning (fork/exec)
- [ ] SIGCHLD handling
- [ ] Compile + test

**Jour 10-11: fs_service**
- [ ] Créer fs_service/src/main.rs (minimal)
- [ ] Mount daemon (future: IPC)
- [ ] Compile + test integration

**Jour 12: Shell completion**
- [ ] Add missing commands (cp, mv, echo, ps, kill)
- [ ] Fix any bugs
- [ ] Integration test with init/fs_service

**Jour 13: Integration**
- [ ] Full boot: kernel → init → fs_service → shell
- [ ] Test all shell commands work
- [ ] Fix mount/filesystem issues

---

### Phase 1c: Test Binaries (1 semaine)

**Jours 14-15: test_hello + test_args**
- [ ] Compile with musl-gcc
- [ ] Test via exec() from shell
- [ ] Verify argv/envp passing

**Jours 16-17: test_fork + test_pipe**
- [ ] Test fork/wait working
- [ ] Test pipe IPC working
- [ ] Verify process trees

**Jours 18-19: test_signals + test_mount**
- [ ] Test signal delivery
- [ ] Test real mount/umount
- [ ] Verify filesystem isolation

**Day 20: Full integration test**
- [ ] All tests pass from shell
- [ ] All I/O working
- [ ] All core functionality verified

---

## ✅ CHECKLIST FINAL

### Before Starting Phase 1b (init):

- [ ] FdTable fully implemented in kernel
- [ ] sys_read/write/open/close connected to VFS
- [ ] exec() loads ELF from VFS correctly
- [ ] Test: `exec /bin/hello` prints "Hello"
- [ ] Test: `/test_args x y z` shows arguments

### Kernel Readiness:

- [ ] 233/333 TODOs resolved (70% of critical ones)
- [ ] No panic() in normal operation
- [ ] No memory leaks (basic valgrind check)
- [ ] QEMU boots stable with init

### Userspace Readiness:

- [ ] init compiles without errors
- [ ] fs_service compiles without errors
- [ ] shell compiles without errors
- [ ] All test binaries compile with musl-gcc
- [ ] Boot sequence: kernel → init → shell works

### Code Quality:

- [ ] **ZERO stubs** in init/fs_service/tests
- [ ] **ZERO TODOs** in new userspace code
- [ ] All functions documented (comments)
- [ ] All error cases handled
- [ ] No undefined behavior (no unsafe without reason)

---

## 📚 RESSOURCES

**Kernel APIs to use:**

```
// Process
fork(), exec(), wait4(), exit()
getpid(), getppid()

// I/O
read(), write(), open(), close()
mkdir(), rmdir(), stat(), fstat()

// Signals
signal(), kill(), pause()

// Filesystem
mount(), umount()
chdir(), getcwd()

// IPC (Phase 2)
pipe(), socket()
```

**musl-gcc to compile userspace:**

```bash
musl-gcc -static -o binary source.c
# OR
musl-gcc -static -c source.c
musl-ar rcs libapp.a source.o
```

---

**End of Phase 1 Preparation - Ready to Build! ✓**

