# POSIX-X Developer Guide

## Table of Contents

1. [Getting Started](#getting-started)
2. [Adding New Syscalls](#adding-new-syscalls)
3. [Testing](#testing)
4. [Debugging](#debugging)
5. [Performance Optimization](#performance-optimization)
6. [Common Patterns](#common-patterns)
7. [Contributing](#contributing)

---

## Getting Started

### Project Structure

```
kernel/src/
├── syscall/
│   ├── dispatch.rs          # Syscall table and dispatch
│   └── handlers/            # Syscall implementations
│       ├── mod.rs           # Handler registration
│       ├── process.rs       # Process management
│       ├── memory.rs        # Memory operations
│       ├── fs_*.rs          # Filesystem operations
│       ├── net_socket.rs    # Networking
│       └── ...
├── posix_x/
│   ├── vfs_posix/           # VFS integration
│   │   ├── path_resolver.rs
│   │   ├── file_ops.rs
│   │   └── inode_cache.rs
│   ├── kernel_interface/   # Kernel bridge
│   │   ├── fd_table.rs
│   │   ├── signal_daemon.rs
│   │   └── ipc_bridge.rs
│   ├── signals/             # Signal handling
│   └── doc/                 # Documentation
```

### Building

```bash
# Build kernel library
cd kernel
cargo build --lib

# Run tests
cargo test --lib

# Check for errors
cargo check --lib

# Format code
cargo fmt

# Run clippy
cargo clippy
```

---

## Adding New Syscalls

### Step-by-Step Guide

#### 1. Define Syscall Number

Edit `kernel/src/syscall/dispatch.rs`:

```rust
pub mod consts {
    // ... existing syscalls
    
    pub const SYS_MY_NEW_SYSCALL: usize = 999;
}
```

**Tip**: Use Linux syscall numbers for compatibility. Check: <https://filippo.io/linux-syscall-table/>

#### 2. Implement Handler

Choose appropriate handler file or create new one in `kernel/src/syscall/handlers/`:

```rust
// In appropriate handler file (e.g., fs_ops.rs for file operations)

/// My new syscall implementation
pub unsafe fn sys_my_new_syscall(
    arg1: i32,
    arg2: *const u8,
    arg3: usize
) -> i64 {
    log::info!("sys_my_new_syscall: arg1={}, arg2={:?}, arg3={}",
               arg1, arg2, arg3);
    
    // 1. Validate parameters
    if arg2.is_null() {
        return -14; // EFAULT
    }
    
    if arg3 > MAX_SIZE {
        return -22; // EINVAL
    }
    
    // 2. Perform operation
    match perform_operation(arg1, arg2, arg3) {
        Ok(result) => result as i64,
        Err(e) => e.to_errno(),
    }
}

fn perform_operation(
    arg1: i32,
    arg2: *const u8,
    arg3: usize
) -> Result<usize> {
    // Implementation here
    Ok(0)
}
```

#### 3. Register Syscall

Edit `kernel/src/syscall/handlers/mod.rs`:

```rust
pub fn initialize_syscall_handlers() {
    // ... existing registrations
    
    let _ = register_syscall(SYS_MY_NEW_SYSCALL, |args| {
        let arg1 = args[0] as i32;
        let arg2 = args[1] as *const u8;
        let arg3 = args[2] as usize;
        let res = unsafe { my_module::sys_my_new_syscall(arg1, arg2, arg3) };
        Ok(res as u64)
    });
}
```

#### 4. Test

Create test in `kernel/tests/syscall_tests.rs`:

```rust
#[test]
fn test_my_new_syscall() {
    unsafe {
        let result = sys_my_new_syscall(42, test_buf.as_ptr(), 100);
        assert_eq!(result, expected_value);
    }
}
```

### Example: Adding `fstatat()`

```rust
// 1. dispatch.rs
pub const SYS_FSTATAT: usize = 262;

// 2. fs_metadata.rs
pub unsafe fn sys_fstatat(
    dirfd: i32,
    pathname: *const i8,
    statbuf: *mut Stat,
    flags: i32
) -> i64 {
    if statbuf.is_null() {
        return -14; // EFAULT
    }
    
    // Resolve path relative to dirfd
    let inode = if pathname.is_null() || *pathname == 0 {
        // Use dirfd itself
        get_inode_from_fd(dirfd)?
    } else {
        let path_str = ptr_to_str(pathname)?;
        if path_str.starts_with('/') {
            // Absolute path, ignore dirfd
            resolve_path(pathname, flags & AT_SYMLINK_NOFOLLOW == 0)?
        } else {
            // Relative to dirfd
            resolve_at(dirfd, pathname, flags & AT_SYMLINK_NOFOLLOW == 0)?
        }
    };
    
    // Get metadata
    fill_stat_buf(inode, statbuf)?;
    
    0
}

// 3. handlers/mod.rs
let _ = register_syscall(SYS_FSTATAT, |args| {
    let dirfd = args[0] as i32;
    let pathname = args[1] as *const i8;
    let statbuf = args[2] as *mut Stat;
    let flags = args[3] as i32;
    let res = unsafe { fs_metadata::sys_fstatat(dirfd, pathname, statbuf, flags) };
    Ok(res as u64)
});
```

---

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_path_resolution() {
        let result = resolve_path(c_str!("/home/user/file.txt"), true);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_invalid_fd() {
        let result = unsafe { sys_read_impl(-1, ptr::null_mut(), 100) };
        assert_eq!(result, Err(VfsError::BadFileDescriptor));
    }
}
```

### Integration Tests

```rust
// tests/integration/syscall_integration.rs

#[test]
fn test_fork_exec_wait() {
    // Test complete process lifecycle
    let pid = fork();
    if pid == 0 {
        execve(...);
    } else {
        let status = wait4(pid, ...);
        assert!(status_ok(status));
    }
}
```

### User-Space Tests

```c
// Create test programs in tests/posix/

#include <stdio.h>
#include <unistd.h>
#include <assert.h>

int main() {
    // Test pipe functionality
    int pipefd[2];
    assert(pipe(pipefd) == 0);
    
    char buf[10] = "test";
    write(pipefd[1], buf, 4);
    
    char readbuf[10];
    assert(read(pipefd[0], readbuf, 4) == 4);
    assert(memcmp(buf, readbuf, 4) == 0);
    
    close(pipefd[0]);
    close(pipefd[1]);
    
    printf("PASS: pipe test\n");
    return 0;
}
```

---

## Debugging

### Logging

```rust
// Use log macros
log::trace!("Detailed trace info");
log::debug!("Debug information");
log::info!("General information");
log::warn!("Warning message");
log::error!("Error occurred");

// Example
pub unsafe fn sys_open_impl(path: *const i8, flags: i32) -> Result<i32> {
    let path_str = ptr_to_str(path)?;
    log::debug!("sys_open: path='{}', flags={:#x}", path_str, flags);
    
    // ...
    
    log::info!("sys_open: opened '{}' as fd {}", path_str, fd);
    Ok(fd)
}
```

### GDB Debugging

```bash
# Start QEMU with GDB server
make qemu-gdb

# In another terminal
gdb kernel/target/x86_64-unknown-none/debug/exo-kernel
(gdb) target remote :1234
(gdb) break sys_open_impl
(gdb) continue
```

### Common Issues

#### Null Pointer Dereference

```rust
// Bad
unsafe fn bad_example(ptr: *const u8) -> u8 {
    *ptr  // Crash if null!
}

// Good
unsafe fn good_example(ptr: *const u8) -> Result<u8> {
    if ptr.is_null() {
        return Err(Error::NullPointer);
    }
    Ok(*ptr)
}
```

#### Use After Free

```rust
// Bad
let handle = fd_table.remove(fd)?;
drop(handle);
handle.inode.read();  // Use after free!

// Good
let handle = fd_table.get(fd)?;
let data = handle.inode.read()?;
// handle still valid
```

#### Deadlock

```rust
// Bad
let lock1 = RESOURCE1.lock();
let lock2 = RESOURCE2.lock();  // Deadlock if another thread locks in reverse order

// Good: Always acquire in same order
let lock1 = RESOURCE1.lock();
let lock2 = RESOURCE2.lock();
```

---

## Performance Optimization

### Benchmarking

```rust
use std::time::Instant;

let start = Instant::now();
for _ in 0..10000 {
    sys_getpid();
}
let duration = start.elapsed();
println!("10000 getpid calls: {:?}", duration);
```

### Optimization Techniques

#### 1. Reduce Lock Contention

```rust
// Bad: Single global lock
static GLOBAL_DATA: Mutex<HashMap<K, V>> = ...;

// Good: Sharded locks
static SHARDED_DATA: [Mutex<HashMap<K, V>>; 16] = ...;

fn get_shard(key: &K) -> &Mutex<HashMap<K, V>> {
    let hash = hash(key);
    &SHARDED_DATA[hash % 16]
}
```

#### 2. Use RwLock for Read-Heavy Workloads

```rust
// Bad: Exclusive lock even for reads
static DATA: Mutex<Vec<Entry>> = ...;

// Good: Shared reads, exclusive writes
static DATA: RwLock<Vec<Entry>> = ...;

// Many readers can access simultaneously
let data = DATA.read().unwrap();
```

#### 3. Cache Frequently Accessed Data

```rust
static INODE_CACHE: Lazy<Mutex<LruCache<String, Inode>>> = 
    Lazy::new(|| Mutex::new(LruCache::new(1000)));

fn get_inode(path: &str) -> Result<Inode> {
    // Check cache first
    if let Some(inode) = INODE_CACHE.lock().get(path) {
        return Ok(inode.clone());
    }
    
    // Expensive lookup
    let inode = resolve_path(path)?;
    
    // Cache for future
    INODE_CACHE.lock().put(path.to_string(), inode.clone());
    
    Ok(inode)
}
```

#### 4. Batch Operations

```rust
// Bad: One syscall per byte
for byte in data {
    write(fd, &byte, 1);
}

// Good: Single syscall
write(fd, data.as_ptr(), data.len());
```

---

## Common Patterns

### Pattern 1: Parameter Validation

```rust
pub unsafe fn sys_example(
    fd: i32,
    buf: *mut u8,
    len: usize
) -> i64 {
    // 1. Validate FD
    if fd < 0 {
        return -9; // EBADF
    }
    
    // 2. Validate buffer
    if buf.is_null() {
        return -14; // EFAULT
    }
    
    // 3. Validate size
    if len > MAX_SIZE {
        return -22; // EINVAL
    }
    
    // 4. Proceed with operation
    // ...
}
```

### Pattern 2: Resource Cleanup

```rust
pub fn allocate_resources() -> Result<ResourceHandle> {
    let resource1 = allocate_resource1()?;
    
    let resource2 = match allocate_resource2() {
        Ok(r) => r,
        Err(e) => {
            // Clean up resource1 before returning error
            free_resource1(resource1);
            return Err(e);
        }
    };
    
    Ok(ResourceHandle { resource1, resource2 })
}

// Better: Use Drop
struct ResourceHandle {
    resource1: Resource1,
    resource2: Resource2,
}

impl Drop for ResourceHandle {
    fn drop(&mut self) {
        // Automatic cleanup
    }
}
```

### Pattern 3: Error Conversion

```rust
impl From<VfsError> for SyscallError {
    fn from(err: VfsError) -> Self {
        match err {
            VfsError::NotFound => SyscallError::NotFound,
            VfsError::PermissionDenied => SyscallError::PermissionDenied,
            // ...
        }
    }
}

// Usage
pub fn syscall_handler() -> i64 {
    match vfs_operation() {
        Ok(result) => result as i64,
        Err(e) => SyscallError::from(e).to_errno(),
    }
}
```

---

## Contributing

### Code Style

Follow Rust conventions:

```rust
// Good naming
fn calculate_size() -> usize { ... }
const MAX_BUFFER_SIZE: usize = 4096;
struct FileDescriptor { ... }

// Document public APIs
/// Opens a file and returns a file descriptor.
///
/// # Arguments
/// * `path` - Path to the file
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, etc.)
///
/// # Returns
/// File descriptor on success, negative errno on error
pub unsafe fn sys_open(path: *const i8, flags: i32) -> i64 {
    ...
}

// Use meaningful variable names
let file_descriptor = open_file(path)?;
let bytes_read = read_from_file(fd, buffer)?;
```

### Pull Request Checklist

- [ ] Code compiles without warnings
- [ ] All tests pass
- [ ] New functionality has tests
- [ ] Documentation updated
- [ ] CHANGELOG.md updated
- [ ] Code formatted with `cargo fmt`
- [ ] No clippy warnings

### Commit Messages

```
feat(syscall): Add fstatat() implementation

- Implement fstatat() syscall (262)
- Support AT_SYMLINK_NOFOLLOW flag
- Add tests for relative path resolution
- Update SYSCALL_REFERENCE.md

Fixes #123
```

---

## Troubleshooting

### Common Compilation Errors

**Error**: `cannot find value 'SYS_MYSYSCALL' in module 'dispatch'`

**Solution**: Add syscall constant to `dispatch.rs`

---

**Error**: `the trait 'Send' is not implemented for [type]`

**Solution**: Ensure type is Send/Sync or wrap in Arc

---

**Error**: `cannot borrow ... as mutable`

**Solution**: Use RwLock or RefCell for interior mutability

---

### Runtime Issues

**Issue**: Syscall returns -14 (EFAULT)

**Check**:

- Null pointer checks
- Buffer validation
- Address validity

---

**Issue**: Deadlock

**Check**:

- Lock ordering
- Locks held across await points
- Recursive locking

---

**Issue**: Memory leak

**Check**:

- Drop implementations
- Reference cycles (Arc cycles)
- FD table cleanup

---

## Resources

### Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md) - System architecture
- [SYSCALL_REFERENCE.md](SYSCALL_REFERENCE.md) - All syscalls
- [VFS_GUIDE.md](VFS_GUIDE.md) - Filesystem operations
- [IPC_GUIDE.md](IPC_GUIDE.md) - IPC mechanisms

### External References

- [Linux Syscall Table](https://filippo.io/linux-syscall-table/)
- [POSIX.1-2017](https://pubs.opengroup.org/onlinepubs/9699919799/)
- [Linux man pages](https://man7.org/linux/man-pages/)

### Tools

- GDB - Debugging
- strace - System call tracing (for comparison)
- perf - Performance analysis
- valgrind - Memory debugging

---

## Summary

This guide covers:

- ✅ Adding new syscalls (4 steps)
- ✅ Testing strategies
- ✅ Debugging techniques
- ✅ Performance optimization
- ✅ Common patterns
- ✅ Contributing guidelines

For questions or issues, refer to the documentation or create an issue on GitHub.

**Happy coding!** 🚀
