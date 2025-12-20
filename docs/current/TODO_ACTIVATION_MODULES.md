# 🎯 TODO - ACTIVATION DES MODULES EXISTANTS

**Date:** 20 décembre 2025  
**Objectif:** Activer les 11,000+ lignes de code désactivées  
**Priorité:** CRITIQUE - Avant toute nouvelle fonctionnalité  
**Durée estimée:** 3-4 semaines

---

## 📋 PLAN D'ACTIVATION EN 6 ÉTAPES

### ✅ ÉTAPE 0: PRÉPARATION (Fait)
- [x] Analyse complète du code
- [x] Identification modules désactivés
- [x] Documentation PHASE_1_REALITY_CHECK.md
- [x] Ce TODO

---

## 🟢 ÉTAPE 1: ACTIVER VFS I/O SYSCALLS (2-3 jours)

**Objectif:** Rendre open/read/write/close/stat fonctionnels

### Tâche 1.1: Décommenter Module I/O

**Fichier:** `kernel/src/syscall/handlers/mod.rs`

```rust
// AVANT (ligne 16)
// ⏸️ Phase 1b: pub mod io;

// APRÈS
pub mod io;
```

**Fichier:** `kernel/src/syscall/handlers/mod.rs` (ligne 30)

```rust
// AVANT
// ⏸️ Phase 1b: pub use io::{Fd, FileFlags, FileStat};

// APRÈS
pub use io::{Fd, FileFlags, FileStat};
```

**Fichier:** `kernel/src/syscall/handlers/io.rs` (ligne 6)

```rust
// AVANT
// ⏸️ Phase 1b: use crate::fs::{vfs, FsError};

// APRÈS
use crate::fs::{vfs, FsError};
```

### Tâche 1.2: Enregistrer Syscalls I/O

**Fichier:** `kernel/src/syscall/handlers/mod.rs` dans `init()`

Ajouter après les syscalls process (ligne ~95):

```rust
// VFS I/O syscalls
let _ = register_syscall(SYS_OPEN, |args| {
    let path_ptr = args[0] as *const u8;
    let flags = args[1] as u32;
    let mode = args[2] as u32;
    
    let path = unsafe { 
        // Read path from user memory
        core::str::from_utf8_unchecked(
            core::slice::from_raw_parts(path_ptr, 256)
        ).trim_end_matches('\0')
    };
    
    match io::sys_open(path, flags, mode) {
        Ok(fd) => Ok(fd as u64),
        Err(e) => Err(memory_err_to_syscall_err(e)),
    }
});

let _ = register_syscall(SYS_READ, |args| {
    let fd = args[0] as i32;
    let buf_ptr = args[1] as *mut u8;
    let count = args[2] as usize;
    
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
    
    match io::sys_read(fd, buf) {
        Ok(n) => Ok(n as u64),
        Err(e) => Err(memory_err_to_syscall_err(e)),
    }
});

let _ = register_syscall(SYS_WRITE, |args| {
    let fd = args[0] as i32;
    let buf_ptr = args[1] as *const u8;
    let count = args[2] as usize;
    
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
    
    match io::sys_write(fd, buf) {
        Ok(n) => Ok(n as u64),
        Err(e) => Err(memory_err_to_syscall_err(e)),
    }
});

let _ = register_syscall(SYS_CLOSE, |args| {
    let fd = args[0] as i32;
    
    match io::sys_close(fd) {
        Ok(_) => Ok(0),
        Err(e) => Err(memory_err_to_syscall_err(e)),
    }
});

let _ = register_syscall(SYS_LSEEK, |args| {
    let fd = args[0] as i32;
    let offset = args[1] as i64;
    let whence = args[2] as i32;
    
    match io::sys_lseek(fd, offset, whence) {
        Ok(new_offset) => Ok(new_offset as u64),
        Err(e) => Err(memory_err_to_syscall_err(e)),
    }
});

log::info!("  ✅ VFS I/O syscalls: open, read, write, close, lseek");
```

### Tâche 1.3: Tests I/O

**Fichier:** `kernel/src/lib.rs` dans `test_fork_thread_entry()`

Ajouter après tests Phase 1a:

```rust
fn test_vfs_io() {
    logger::early_print("\n[TEST] VFS I/O Integration Test\n");
    
    // Test 1: Open file for writing
    let fd = match io::sys_open("/tmp/test.txt", O_WRONLY | O_CREAT, 0o644) {
        Ok(fd) => {
            logger::early_print("[TEST 1] ✅ open() for write succeeded\n");
            fd
        }
        Err(_) => {
            logger::early_print("[TEST 1] ❌ open() failed\n");
            return;
        }
    };
    
    // Test 2: Write data
    let data = b"Hello from VFS I/O syscalls!";
    match io::sys_write(fd, data) {
        Ok(n) if n == data.len() => {
            logger::early_print("[TEST 2] ✅ write() succeeded\n");
        }
        _ => {
            logger::early_print("[TEST 2] ❌ write() failed\n");
            return;
        }
    }
    
    // Test 3: Close file
    match io::sys_close(fd) {
        Ok(_) => logger::early_print("[TEST 3] ✅ close() succeeded\n"),
        Err(_) => logger::early_print("[TEST 3] ❌ close() failed\n"),
    }
    
    // Test 4: Re-open for reading
    let fd = match io::sys_open("/tmp/test.txt", O_RDONLY, 0) {
        Ok(fd) => {
            logger::early_print("[TEST 4] ✅ open() for read succeeded\n");
            fd
        }
        Err(_) => {
            logger::early_print("[TEST 4] ❌ open() failed\n");
            return;
        }
    };
    
    // Test 5: Read data back
    let mut buf = [0u8; 128];
    match io::sys_read(fd, &mut buf) {
        Ok(n) if n == data.len() && &buf[..n] == data => {
            logger::early_print("[TEST 5] ✅ read() data matches!\n");
        }
        _ => {
            logger::early_print("[TEST 5] ❌ read() data mismatch\n");
        }
    }
    
    io::sys_close(fd).ok();
    logger::early_print("[TEST] ✅ VFS I/O Integration PASSED\n\n");
}
```

### Tâche 1.4: Validation

```bash
cd kernel
cargo build --target ../x86_64-unknown-none.json
cd ..
bash docs/scripts/build.sh
qemu-system-x86_64 -cdrom build/exo_os.iso -m 512M -serial stdio -display none
```

**Critères de succès:**
- ✅ Compilation sans erreur
- ✅ Boot QEMU
- ✅ Tests VFS I/O PASSED
- ✅ Logs montrent "✅ VFS I/O syscalls"

---

## 🟡 ÉTAPE 2: ACTIVER FILESYSTEM OPERATIONS (3-4 jours)

**Objectif:** Activer mkdir/stat/chmod/link/poll/futex

### Tâche 2.1: Décommenter 8 Modules FS

**Fichier:** `kernel/src/syscall/handlers/mod.rs`

```rust
// AVANT
// ⏸️ Phase 1b: pub mod fs_dir;
// ⏸️ Phase 1b: pub mod fs_events;
// ⏸️ Phase 1b: pub mod fs_fcntl;
// ⏸️ Phase 1b: pub mod fs_fifo;
// ⏸️ Phase 1b: pub mod fs_futex;
// ⏸️ Phase 1b: pub mod fs_link;
// ⏸️ Phase 1b: pub mod fs_ops;
// ⏸️ Phase 1b: pub mod fs_poll;
// ⏸️ Phase 1b: pub mod inotify;

// APRÈS
pub mod fs_dir;
pub mod fs_events;
pub mod fs_fcntl;
pub mod fs_fifo;
pub mod fs_futex;
pub mod fs_link;
pub mod fs_ops;
pub mod fs_poll;
pub mod inotify;
```

### Tâche 2.2: Corriger Imports VFS

Chaque module `fs_*.rs` a des imports commentés. Les décommenter:

**Fichier:** `kernel/src/syscall/handlers/fs_dir.rs` (ligne 8-9)

```rust
// AVANT
// ⏸️ Phase 1b: use crate::fs::vfs::inode::InodeType;
// ⏸️ Phase 1b: use crate::fs::{FsError, FsResult};

// APRÈS
use crate::fs::vfs::inode::InodeType;
use crate::fs::{FsError, FsResult};
```

**Répéter pour:**
- `fs_fifo.rs`
- `fs_link.rs`
- `fs_ops.rs`

### Tâche 2.3: Remplacer Stub sys_link()

**Fichier:** `kernel/src/syscall/handlers/fs_link.rs` (ligne 17-20)

```rust
// AVANT
pub fn sys_link(_oldpath: &str, _newpath: &str) -> i64 {
    -38 // ENOSYS
}

// APRÈS
pub fn sys_link(oldpath: &str, newpath: &str) -> FsResult<()> {
    // Get source inode
    let src_inode = vfs::lookup(oldpath)?;
    
    // Check if it's a directory (can't hard link dirs)
    if src_inode.read().inode_type() == InodeType::Directory {
        return Err(FsError::IsDirectory);
    }
    
    // Create hard link in VFS
    vfs::link(oldpath, newpath)?;
    
    Ok(())
}
```

### Tâche 2.4: Enregistrer Syscalls FS

**Fichier:** `kernel/src/syscall/handlers/mod.rs` dans `init()`

```rust
// Filesystem operations
let _ = register_syscall(SYS_MKDIR, |args| {
    let path_ptr = args[0] as *const u8;
    let mode = args[1] as u32;
    // ... parse path, call fs_dir::sys_mkdir()
});

let _ = register_syscall(SYS_STAT, |args| {
    // ... call fs_ops::sys_stat()
});

let _ = register_syscall(SYS_FSTAT, |args| {
    // ... call fs_ops::sys_fstat()
});

let _ = register_syscall(SYS_CHMOD, |args| {
    // ... call fs_ops::sys_chmod()
});

let _ = register_syscall(SYS_LINK, |args| {
    // ... call fs_link::sys_link()
});

let _ = register_syscall(SYS_SYMLINK, |args| {
    // ... call fs_link::sys_symlink()
});

let _ = register_syscall(SYS_POLL, |args| {
    // ... call fs_poll::sys_poll()
});

let _ = register_syscall(SYS_FUTEX, |args| {
    // ... call fs_futex::sys_futex()
});

log::info!("  ✅ Filesystem ops: mkdir, stat, chmod, link, poll, futex");
```

### Tâche 2.5: Tests FS Ops

```rust
fn test_fs_ops() {
    // Test mkdir
    fs_dir::sys_mkdir("/tmp/testdir", 0o755).expect("mkdir failed");
    
    // Test stat
    let stat = fs_ops::sys_stat("/tmp/testdir").expect("stat failed");
    assert!(stat.mode & 0o040000 != 0); // Directory bit
    
    // Test link
    io::sys_open("/tmp/file1", O_CREAT, 0o644).ok();
    fs_link::sys_link("/tmp/file1", "/tmp/file2").expect("link failed");
    
    logger::early_print("[TEST] ✅ Filesystem ops PASSED\n");
}
```

---

## 🟡 ÉTAPE 3: ACTIVER ELF LOADER + EXEC (2-3 jours)

**Objectif:** Permettre exec() avec binaires ELF réels

### Tâche 3.1: Activer Module Loader

**Fichier:** `kernel/src/lib.rs` (ligne 47)

```rust
// AVANT
// pub mod loader;      // ⏸️ Phase 1b: ELF loader

// APRÈS
pub mod loader;
```

### Tâche 3.2: Compléter sys_execve()

**Fichier:** `kernel/src/syscall/handlers/process.rs` (lignes 279-290)

```rust
// AVANT (stub commenté)
// ⏸️ Phase 1b: VFS not loaded
pub fn sys_execve(/* ... */) -> MemoryResult<i64> {
    /* Phase 1b implementation:
    let bin = vfs::read_file(filename)?;
    let loaded = loader::load_elf(&bin)?;
    // ... */
    Err(MemoryError::NotSupported)
}

// APRÈS
pub fn sys_execve(
    filename: &str,
    argv: &[&str],
    envp: &[&str],
) -> MemoryResult<i64> {
    log::info!("[EXECVE] Loading {}", filename);
    
    // Read ELF binary from VFS
    let bin_data = crate::fs::vfs::read_file(filename)
        .map_err(|_| MemoryError::NotFound)?;
    
    // Parse and load ELF
    let loaded = crate::loader::load_elf(&bin_data)?;
    
    // Get current process
    let current = current_process();
    
    // Replace address space
    current.mm.clear()?;
    
    // Map segments
    for segment in &loaded.segments {
        current.mm.map_region(
            segment.vaddr,
            segment.size,
            segment.flags,
        )?;
        
        // Copy data
        unsafe {
            core::ptr::copy_nonoverlapping(
                segment.data.as_ptr(),
                segment.vaddr as *mut u8,
                segment.data.len(),
            );
        }
    }
    
    // Setup stack
    let stack_top = 0x7FFF_FFFF_F000;
    let stack_size = 8 * 1024 * 1024; // 8MB
    current.mm.map_stack(stack_top, stack_size)?;
    
    // Push argv/envp on stack
    let sp = setup_user_stack(stack_top, argv, envp)?;
    
    // Setup registers for user mode entry
    let mut regs = UserRegs::default();
    regs.rip = loaded.entry_point;
    regs.rsp = sp;
    regs.rdi = argv.len() as u64; // argc
    regs.rsi = sp + 8; // argv pointer
    
    // Jump to user mode
    jump_to_user(&regs);
    
    // Never returns
    unreachable!()
}
```

### Tâche 3.3: Build Test Binaries

**Script:** `scripts/build_test_binaries.sh` (déjà existant)

```bash
#!/bin/bash
# Build simple test binaries with musl

set -e

mkdir -p userland/bin

# Test 1: Hello World
cat > userland/hello.c << 'EOF'
#include <stdio.h>
int main() {
    printf("Hello from Exo-OS userspace!\n");
    return 0;
}
EOF

musl-gcc -static -o userland/bin/hello userland/hello.c

# Test 2: Test Args
cat > userland/test_args.c << 'EOF'
#include <stdio.h>
int main(int argc, char** argv) {
    printf("argc = %d\n", argc);
    for (int i = 0; i < argc; i++) {
        printf("argv[%d] = %s\n", i, argv[i]);
    }
    return 0;
}
EOF

musl-gcc -static -o userland/bin/test_args userland/test_args.c

# Test 3: Fork+Exec
cat > userland/test_fork_exec.c << 'EOF'
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

int main() {
    pid_t pid = fork();
    if (pid == 0) {
        // Child
        execve("/bin/hello", NULL, NULL);
    } else {
        // Parent
        int status;
        wait(&status);
        printf("Child exited with status %d\n", status);
    }
    return 0;
}
EOF

musl-gcc -static -o userland/bin/test_fork_exec userland/test_fork_exec.c

echo "✅ Test binaries built in userland/bin/"
```

**Exécuter:**

```bash
chmod +x scripts/build_test_binaries.sh
./scripts/build_test_binaries.sh
```

### Tâche 3.4: Intégrer Binaries dans tmpfs

**Fichier:** `kernel/src/boot/late_init.rs` dans `init_vfs()`

```rust
fn init_vfs() -> Result<(), &'static str> {
    // ... existing tmpfs mount ...
    
    // Load test binaries into tmpfs
    vfs::create_dir("/bin").ok();
    
    // Embed hello binary
    const HELLO_BIN: &[u8] = include_bytes!("../../userland/bin/hello");
    let handle = vfs::create_file("/bin/hello")?;
    vfs::write(handle, HELLO_BIN)?;
    vfs::close(handle)?;
    
    log::info!("  ✅ Test binaries loaded into /bin/");
    Ok(())
}
```

### Tâche 3.5: Test exec()

```rust
fn test_exec() {
    logger::early_print("\n[TEST] exec() Test\n");
    
    // Fork child
    let pid = process::sys_fork().expect("fork failed");
    
    if pid == 0 {
        // Child process - exec hello
        logger::early_print("[CHILD] Executing /bin/hello\n");
        process::sys_execve("/bin/hello", &[], &[]).ok();
        // Should not return
        logger::early_print("[CHILD] ❌ execve returned!\n");
    } else {
        // Parent - wait for child
        let mut status = 0;
        process::sys_wait(pid, WaitOptions::default()).ok();
        logger::early_print("[TEST] ✅ exec() test PASSED\n");
    }
}
```

---

## 🟡 ÉTAPE 4: ACTIVER SHELL INTERACTIF (2-3 jours)

**Objectif:** Shell fonctionnel avec clavier

### Tâche 4.1: Activer Module Shell

**Fichier:** `kernel/src/lib.rs` (ligne 48)

```rust
// AVANT
// pub mod shell;       // ⏸️ Phase 1b: Interactive shell

// APRÈS
pub mod shell;
```

### Tâche 4.2: Implémenter Shell Basique

**Créer:** `kernel/src/shell/mod.rs`

```rust
//! Interactive Shell for Exo-OS

use crate::drivers::char::keyboard::read_line;
use crate::syscall::handlers::{io, process};
use alloc::string::String;
use alloc::vec::Vec;

pub fn shell_loop() {
    println!("Exo-OS Shell v0.1");
    println!("Type 'help' for commands\n");
    
    loop {
        print!("exo$ ");
        
        let line = read_line();
        let parts: Vec<&str> = line.split_whitespace().collect();
        
        if parts.is_empty() {
            continue;
        }
        
        match parts[0] {
            "help" => cmd_help(),
            "ls" => cmd_ls(parts.get(1).unwrap_or(&"/")),
            "cat" => cmd_cat(parts.get(1)),
            "echo" => cmd_echo(&parts[1..]),
            "exit" => break,
            cmd => {
                // Try to execute as binary
                execute_binary(cmd, &parts[1..]);
            }
        }
    }
}

fn execute_binary(cmd: &str, args: &[&str]) {
    let path = format!("/bin/{}", cmd);
    
    let pid = match process::sys_fork() {
        Ok(p) => p,
        Err(_) => {
            println!("Error: fork failed");
            return;
        }
    };
    
    if pid == 0 {
        // Child - exec
        process::sys_execve(&path, args, &[]).ok();
        println!("Error: exec failed");
        process::sys_exit(1);
    } else {
        // Parent - wait
        process::sys_wait(pid, process::WaitOptions::default()).ok();
    }
}
```

### Tâche 4.3: Intégrer dans Boot

**Fichier:** `kernel/src/lib.rs` dans `rust_main()`

```rust
// Après init et tests
logger::early_print("[KERNEL] Entering shell...\n\n");
crate::shell::shell_loop();
```

---

## 🟠 ÉTAPE 5: ACTIVER POSIX-X SYSCALLS (4-5 jours)

**Objectif:** Enregistrer les 141 syscalls POSIX-X

### Tâche 5.1: Activer Modules POSIX-X

**Fichier:** `kernel/src/posix_x/mod.rs`

```rust
// AVANT
// ⏸️ Phase 1b: pub mod syscalls;
// ⏸️ Phase 1b: pub mod vfs_posix;
// ⏸️ Phase 1b: pub use vfs_posix::{file_ops, VfsHandle};

// APRÈS
pub mod syscalls;
pub mod vfs_posix;
pub use vfs_posix::{file_ops, VfsHandle};
```

### Tâche 5.2: Créer Registration Table

**Créer:** `kernel/src/posix_x/syscalls/register.rs`

```rust
//! POSIX-X Syscall Registration
//! 
//! Registers all 141 syscalls with the dispatch table

use crate::syscall::dispatch::{register_syscall, syscall_numbers::*};
use super::{fast_path, hybrid_path, legacy_path};

pub fn register_all_posix_x() {
    log::info!("[POSIX-X] Registering 141 syscalls...");
    
    // Fast path (11 syscalls)
    register_fast_path();
    
    // Hybrid path (80+ syscalls)
    register_hybrid_path();
    
    // Legacy path (40+ syscalls)
    register_legacy_path();
    
    log::info!("[POSIX-X] ✅ All syscalls registered");
}

fn register_fast_path() {
    // getpid, gettid, getuid, getgid, etc.
    let _ = register_syscall(SYS_GETPID, |_| {
        Ok(fast_path::sys_getpid())
    });
    
    let _ = register_syscall(SYS_GETTID, |_| {
        Ok(fast_path::sys_gettid())
    });
    
    // ... (11 syscalls)
}

fn register_hybrid_path() {
    // I/O, sockets, memory, signals
    // ... (80+ syscalls)
}

fn register_legacy_path() {
    // fork, exec, SysV IPC
    // NOTE: Use handlers/process.rs for fork/exec (not stubs)
    // ... (40+ syscalls)
}
```

### Tâche 5.3: Appeler Registration

**Fichier:** `kernel/src/syscall/handlers/mod.rs` dans `init()`

```rust
pub fn init() {
    log::info!("[Phase 1] Registering syscall handlers...");
    
    // ... existing handlers ...
    
    // Register all POSIX-X syscalls
    crate::posix_x::syscalls::register::register_all_posix_x();
    
    log::info!("  ✅ POSIX-X: 141 syscalls registered");
}
```

### Tâche 5.4: Remplacer Stubs ENOSYS

**Fichier:** `kernel/src/posix_x/syscalls/legacy_path/fork.rs`

```rust
// AVANT
pub fn sys_fork() -> i64 { -38 } // ENOSYS

// APRÈS
pub fn sys_fork() -> i64 {
    // Delegate to actual implementation
    match crate::syscall::handlers::process::sys_fork() {
        Ok(pid) => pid as i64,
        Err(_) => -1,
    }
}
```

**Répéter pour:**
- `sys_vfork()` → `sys_fork()`
- `sys_execve()` → `process::sys_execve()`
- `sys_execveat()` → `process::sys_execve()` (avec path resolution)

---

## 🟠 ÉTAPE 6: ACTIVER IPC MODULE (3-4 jours)

**Objectif:** Rendre Fusion Rings utilisables

### Tâche 6.1: Activer Module IPC

**Fichier:** `kernel/src/lib.rs` (ligne 62)

```rust
// AVANT
// pub mod ipc;         // ⏸️ Phase 2: IPC zerocopy

// APRÈS
pub mod ipc;
```

### Tâche 6.2: Créer Syscall Handlers IPC

**Fichier:** `kernel/src/syscall/handlers/ipc.rs` (décommenter)

```rust
//! IPC Syscall Handlers

use crate::ipc::{FusionRing, Message, ChannelHandle};
use crate::memory::MemoryResult;

pub fn sys_ipc_create_channel(name: &str, size: usize) -> MemoryResult<ChannelHandle> {
    crate::ipc::named::create_channel(name, size)
}

pub fn sys_ipc_connect(name: &str) -> MemoryResult<ChannelHandle> {
    crate::ipc::named::connect(name)
}

pub fn sys_ipc_send(handle: ChannelHandle, msg: &[u8]) -> MemoryResult<()> {
    let channel = crate::ipc::named::get_channel(handle)?;
    channel.send(msg)
}

pub fn sys_ipc_recv(handle: ChannelHandle, buf: &mut [u8]) -> MemoryResult<usize> {
    let channel = crate::ipc::named::get_channel(handle)?;
    channel.recv(buf)
}

pub fn sys_ipc_close(handle: ChannelHandle) -> MemoryResult<()> {
    crate::ipc::named::close(handle)
}
```

### Tâche 6.3: Enregistrer Syscalls IPC

**Fichier:** `kernel/src/syscall/handlers/mod.rs`

```rust
// AVANT
// ⏸️ Phase 2: pub mod ipc;

// APRÈS
pub mod ipc;
```

Dans `init()`:

```rust
// IPC syscalls (custom Exo-OS extension)
let _ = register_syscall(500, |args| { // SYS_IPC_CREATE
    // ... sys_ipc_create_channel()
});

let _ = register_syscall(501, |args| { // SYS_IPC_CONNECT
    // ... sys_ipc_connect()
});

// ... autres syscalls IPC

log::info!("  ✅ IPC syscalls: Fusion Rings active");
```

### Tâche 6.4: Tests IPC

```rust
fn test_ipc_fusion_rings() {
    // Create channel
    let handle = ipc::sys_ipc_create_channel("test_channel", 256)
        .expect("create channel failed");
    
    // Send message
    let msg = b"Hello IPC!";
    ipc::sys_ipc_send(handle, msg).expect("send failed");
    
    // Recv message
    let mut buf = [0u8; 128];
    let n = ipc::sys_ipc_recv(handle, &mut buf).expect("recv failed");
    assert_eq!(n, msg.len());
    assert_eq!(&buf[..n], msg);
    
    logger::early_print("[TEST] ✅ IPC Fusion Rings PASSED\n");
}
```

---

## 📊 CHECKLIST FINALE

### Phase 1 - Vraiment Complète

- [ ] **Étape 1:** VFS I/O syscalls actifs (open/read/write/close/stat)
- [ ] **Étape 2:** Filesystem ops actifs (mkdir/chmod/link/poll/futex)
- [ ] **Étape 3:** ELF loader + exec() fonctionnels
- [ ] **Étape 4:** Shell interactif avec clavier
- [ ] **Étape 5:** POSIX-X 141 syscalls enregistrés
- [ ] **Étape 6:** IPC Fusion Rings utilisables

### Critères de Validation

**Tests à passer:**
```
✅ VFS I/O: open → write → close → open → read
✅ FS Ops: mkdir → stat → chmod → link
✅ exec(): fork → execve("/bin/hello") → wait
✅ Shell: Prompt → commande → exécution
✅ POSIX-X: 141 syscalls sans ENOSYS
✅ IPC: create → send → recv → close
```

**Benchmarks:**
```
✅ Context switch: <350 cycles
✅ IPC latency: <400 cycles
✅ Syscall fast path: <100 cycles
```

---

## 🎯 RÉSULTAT ATTENDU

### Après 3-4 Semaines

**Code actif:** ~20,000 lignes (vs ~9,500 actuellement)

**Fonctionnalités:**
- ✅ VFS complet (tmpfs/devfs/procfs)
- ✅ I/O syscalls (open/read/write/stat)
- ✅ Process management (fork/exec/wait)
- ✅ ELF loader (binaires userspace)
- ✅ Shell interactif
- ✅ POSIX-X (141 syscalls)
- ✅ IPC Fusion Rings
- ✅ Filesystem operations complètes

**Phase 1 RÉELLEMENT à 95%+**

---

## 🚀 PROCHAINES ÉTAPES (Phase 2)

Après activation des modules existants:

1. **Network Stack** - Activer `pub mod net;` (3-4 semaines)
2. **SMP Multi-core** - Bootstrap APs (4-5 semaines)
3. **Drivers réels** - PCI, network, block (3-4 semaines)

**Total Phase 2:** 10-13 semaines

---

## 📝 NOTES IMPORTANTES

### Ordre d'Exécution

**IMPÉRATIF:** Suivre l'ordre des étapes:
1. VFS I/O d'abord (dépendance pour tout)
2. FS ops ensuite
3. ELF loader (nécessite VFS)
4. Shell (nécessite exec)
5. POSIX-X (orchestration finale)
6. IPC (indépendant, peut être parallèle)

### Compilation Incrémentale

Après chaque étape:
```bash
cargo build --target ../x86_64-unknown-none.json
bash docs/scripts/build.sh
qemu-system-x86_64 -cdrom build/exo_os.iso -m 512M -serial stdio
```

**Ne jamais passer à l'étape suivante si la précédente échoue.**

### Debug

Si erreurs de compilation:
1. Vérifier tous les imports décommentés
2. Vérifier compatibilité types (MemoryError vs FsError)
3. Vérifier syscall numbers dans dispatch
4. Logs avec `log::debug!` pour tracer

---

**Document de référence pour l'activation complète de la Phase 1.**
