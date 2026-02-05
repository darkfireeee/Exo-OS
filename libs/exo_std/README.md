# exo_std

Standard library for Exo-OS native applications.

## Features

- **Process management**: spawn, fork, exec, wait
- **I/O operations**: File, stdin/stdout/stderr
- **Synchronization**: Mutex, RwLock, Atomic operations
- **Thread primitives**: Thread spawning, joining
- **IPC**: Inter-process communication
- **Time**: Monotonic and realtime clocks
- **Security**: Capability-based security primitives
- **Collections** (planned): RingBuffer, BoundedVec, IntrusiveList, RadixTree
- **Allocators** (planned): See exo_allocator

## Architecture

```
exo_std/
├── src/
│   ├── process.rs      # Process management
│   ├── io.rs           # I/O operations
│   ├── sync.rs         # Synchronization primitives
│   ├── thread.rs       # Thread management
│   ├── ipc.rs          # IPC
│   ├── time.rs         # Time primitives
│   ├── security.rs     # Security APIs
│   └── collections/    # Data structures (planned)
```

## Usage

### Process Management

```rust
use exo_std::process::Command;

let output = Command::new("/bin/ls")
    .args(&["-la"])
    .spawn()?
    .wait()?;
```

### File I/O

```rust
use exo_std::fs::File;

let mut file = File::open("/etc/config")?;
let contents = file.read_to_string()?;
```

### Threading

```rust
use exo_std::thread;

let handle = thread::spawn(|| {
    println!("Hello from thread!");
});
handle.join()?;
```

### Synchronization

```rust
use exo_std::sync::Mutex;

let data = Mutex::new(0);
{
    let mut guard = data.lock();
    *guard += 1;
}
```

## Design Principles

- **No implicit allocations**: Explicit memory management
- **Zero-cost abstractions**: No runtime overhead
- **Type safety**: Leverage Rust's type system
- **Capability-based**: Security by design

## Comparison with std

| Feature | exo_std | std | Notes |
|---------|---------|-----|-------|
| Process | ✓ | ✓ | Custom syscall layer |
| File I/O | ✓ | ✓ | VFS integration |
| Threading | ✓ | ✓ | Kernel threads |
| Networking | See exo_net | ✓ | Separate crate |

## References

- [Rust std Documentation](https://doc.rust-lang.org/std/)
