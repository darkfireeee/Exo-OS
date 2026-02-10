# Quick Reference - IPC & Pseudo Filesystems

## File Locations

```
/workspaces/Exo-OS/kernel/src/fs/
├── ipc/
│   ├── mod.rs         ← IPC module initialization
│   ├── pipefs.rs      ← Named pipes & anonymous pipes
│   ├── socketfs.rs    ← Unix domain sockets
│   └── shmfs.rs       ← Shared memory
└── pseudo/
    ├── mod.rs         ← Pseudo fs initialization
    ├── procfs.rs      ← Process info (/proc)
    ├── sysfs.rs       ← Kernel objects (/sys)
    └── devfs.rs       ← Device nodes (/dev)
```

## Quick API Reference

### PipeFS
```rust
use crate::fs::ipc::pipefs;

// Create anonymous pipe
let (read_end, write_end) = pipefs::pipe_create();

// Create named pipe
let fifo = pipefs::mkfifo("/tmp/my_pipe")?;

// Open existing FIFO
let reader = pipefs::open_fifo("/tmp/my_pipe", false)?; // for reading
let writer = pipefs::open_fifo("/tmp/my_pipe", true)?;  // for writing

// Remove FIFO
pipefs::unlink_fifo("/tmp/my_pipe")?;
```

### SocketFS
```rust
use crate::fs::ipc::socketfs::{socket_create, SocketType, SocketAddr};

// Create stream socket
let sock = socket_create(SocketType::Stream);

// Bind to address
sock.bind(SocketAddr::path("/tmp/my.sock"))?;

// Server: listen and accept
sock.listen(5)?;
let client_sock = sock.accept(false)?;

// Client: connect
sock.connect(SocketAddr::path("/tmp/my.sock"))?;

// Send/receive
sock.send(b"Hello", false)?;
let mut buf = [0u8; 1024];
let n = sock.recv(&mut buf, false)?;

// Datagram socket
let dgram = socket_create(SocketType::Dgram);
dgram.sendto(b"packet", &SocketAddr::path("/tmp/server.sock"))?;
let (n, sender) = dgram.recvfrom(&mut buf, false)?;
```

### ShmFS
```rust
use crate::fs::ipc::shmfs;

// Create shared memory
let shm = shmfs::shm_create("/my_shm", true, true)?;

// Open existing
let shm = shmfs::shm_create("/my_shm", false, false)?;

// Read/Write (via Inode trait)
shm.write_at(0, b"shared data")?;
let mut buf = [0u8; 11];
shm.read_at(0, &mut buf)?;

// Truncate
shm.truncate(4096)?;

// Unlink (object persists until all references dropped)
shmfs::shm_unlink("/my_shm")?;
```

### DevFS
```rust
use crate::fs::pseudo::devfs::{register_device, DeviceType, DeviceOps};

// Get standard device
let null_dev = devfs::get().get_device("null")?;

// Register custom device
struct MyDevice;
impl DeviceOps for MyDevice {
    fn read(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        // Implementation
    }
    fn write(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        // Implementation
    }
}

register_device(
    "mydev",
    DeviceType::Char,
    250, // major
    0,   // minor
    Arc::new(RwLock::new(MyDevice)),
)?;
```

### ProcFS
```rust
use crate::fs::pseudo::procfs;

// Get inode for a procfs entry
let cpuinfo = procfs::get().get_inode("/cpuinfo")?;
let meminfo = procfs::get().get_inode("/meminfo")?;
let proc1_status = procfs::get().get_inode("/1/status")?;

// Read content (via Inode trait)
let mut buf = vec![0u8; 4096];
let n = cpuinfo.read_at(0, &mut buf)?;
```

### SysFS
```rust
use crate::fs::pseudo::sysfs;

// Read hostname
let hostname = sysfs::get_hostname();

// Set hostname
sysfs::set_hostname("exo-machine")?;

// Get inode for sysfs entry
let hostname_inode = sysfs::get().get_inode("/kernel/hostname")?;
```

## Common Patterns

### Blocking I/O with Pipes
```rust
let (reader, writer) = pipefs::pipe_create();

// Writer thread
writer.write_at(0, b"Hello, pipe!")?;

// Reader thread (blocks until data available)
let mut buf = [0u8; 128];
let n = reader.read_at(0, &mut buf)?;
```

### Non-blocking Socket
```rust
let sock = socket_create(SocketType::Stream);
sock.connect(addr)?;

// Send with non-blocking flag
match sock.send(b"data", true) {
    Ok(n) => println!("Sent {} bytes", n),
    Err(FsError::Again) => println!("Would block"),
    Err(e) => return Err(e),
}
```

### Process Information
```rust
// Read process status
let status = procfs::get().get_inode("/1/status")?;
let mut buf = vec![0u8; 1024];
status.read_at(0, &mut buf)?;

// Read system uptime
let uptime = procfs::get().get_inode("/uptime")?;
uptime.read_at(0, &mut buf)?;
```

## Integration with VFS

All filesystems implement the `Inode` trait:

```rust
use crate::fs::core::types::Inode;

fn use_inode(inode: Arc<dyn Inode>) -> FsResult<()> {
    // All IPC and pseudo fs inodes support these:
    let size = inode.size();
    let itype = inode.inode_type();
    let perms = inode.permissions();

    // Read/write
    let mut buf = [0u8; 1024];
    let n = inode.read_at(0, &mut buf)?;
    inode.write_at(0, b"data")?;

    Ok(())
}
```

## Performance Tips

1. **Pipes**: Use large buffers (4KB+) for maximum throughput
2. **Sockets**: Batch small messages to reduce system call overhead
3. **Shared Memory**: Prefer for large data transfers (>1MB)
4. **ProcFS**: Cache frequently read values
5. **DevFS**: Use mmap for /dev/zero when allocating large zero-filled buffers

## Error Handling

All functions return `FsResult<T>` which is `Result<T, FsError>`:

```rust
use crate::fs::{FsError, FsResult};

match operation() {
    Ok(result) => { /* success */ }
    Err(FsError::NotFound) => { /* handle not found */ }
    Err(FsError::Again) => { /* would block */ }
    Err(FsError::PermissionDenied) => { /* permission denied */ }
    Err(e) => { /* other errors */ }
}
```

## Thread Safety

All implementations are thread-safe:
- Multiple readers/writers supported
- Internal synchronization with RwLock/Mutex
- Atomic reference counting with Arc
- Wait queues for blocking operations

## Testing

```rust
#[test]
fn test_pipe_basic() {
    let (r, w) = pipefs::pipe_create();
    w.write_at(0, b"test")?;
    let mut buf = [0u8; 4];
    let n = r.read_at(0, &mut buf)?;
    assert_eq!(n, 4);
    assert_eq!(&buf, b"test");
}

#[test]
fn test_socket_connect() {
    let server = socket_create(SocketType::Stream);
    server.bind(SocketAddr::path("/tmp/test.sock"))?;
    server.listen(1)?;

    let client = socket_create(SocketType::Stream);
    client.connect(SocketAddr::path("/tmp/test.sock"))?;

    let conn = server.accept(false)?;
    client.send(b"hello", false)?;

    let mut buf = [0u8; 5];
    conn.recv(&mut buf, false)?;
    assert_eq!(&buf, b"hello");
}
```

---

For complete documentation, see: `/workspaces/Exo-OS/kernel/src/fs/IPC_PSEUDO_IMPLEMENTATION.md`
