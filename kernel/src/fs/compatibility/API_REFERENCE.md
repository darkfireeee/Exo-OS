# Filesystem Compatibility Layer - API Reference

## Quick Start

```rust
use crate::fs::compatibility::*;
use crate::fs::core::types::InodeType;
```

## tmpfs - In-Memory Filesystem

### Create tmpfs
```rust
// Default (1 GB)
let fs = TmpFs::new();

// Custom size
let fs = TmpFs::new_with_size(512 * 1024 * 1024); // 512 MB
```

### File Operations
```rust
// Create file
let file = fs.create_inode(InodeType::File);

// Write data
let mut guard = file.write();
guard.write_at(0, b"Hello, World!")?;

// Read data
let mut buf = [0u8; 100];
let n = guard.read_at(0, &mut buf)?;

// Get metadata
let size = guard.size();
let perms = guard.permissions();
```

### Directory Operations
```rust
// Create directory
let dir = fs.create_inode(InodeType::Directory);

// Add entry
let file = fs.create_inode(InodeType::File);
dir.write().link("test.txt", file.read().ino())?;

// List directory
let entries = dir.read().list()?;

// Lookup entry
let ino = dir.read().lookup("test.txt")?;
```

### Statistics
```rust
let stats = fs.statfs();
println!("Total: {} bytes", stats.total_size);
println!("Used: {} bytes", stats.used_size);
println!("Files: {}", stats.inode_count);
```

---

## ext4 - Read-Only Compatibility

### Mount ext4 Filesystem
```rust
use crate::fs::block::BlockDevice;

let device: Arc<Mutex<dyn BlockDevice>> = get_device();
let fs = Ext4ReadOnlyFs::mount(device)?;
```

### Read Files
```rust
// Get root directory
let root = fs.root()?;

// List directory
let entries = root.list()?;

// Lookup file
let file_ino = root.lookup("README.txt")?;
let file = fs.read_inode(file_ino)?;

// Read file contents
let mut buf = vec![0u8; file.size() as usize];
file.read_at(0, &mut buf)?;
```

### Navigate Directories
```rust
let root = fs.root()?;
let dir_ino = root.lookup("documents")?;
let dir = fs.read_inode(dir_ino)?;
let entries = dir.read_dir()?;

for entry in entries {
    println!("{}: inode {}", entry.name, entry.inode);
}
```

### Read Symlinks
```rust
let link = fs.read_inode(link_ino)?;
if link.inode_type() == InodeType::Symlink {
    let target = link.readlink()?;
    println!("Link points to: {}", target);
}
```

---

## FAT32 - Full Read/Write Support

### Mount FAT32 Filesystem
```rust
let device: Arc<Mutex<dyn BlockDevice>> = get_usb_device();
let fs = Fat32Fs::mount(device)?;
```

### Directory Operations
```rust
// Get root directory
let root = fs.root();

// Read directory entries
let entries = fs.read_dir(&root)?;
for entry in &entries {
    println!("{}: {} bytes", entry.name, entry.file_size);
    if entry.is_directory() {
        println!("  (directory)");
    }
}

// Find specific entry
let entry = fs.find_entry(&root, "README.TXT")?;
```

### Read Files
```rust
let entry = fs.find_entry(&root, "data.txt")?;

// Read entire file
let data = fs.read_file_all(&entry)?;

// Read partial
let mut buf = vec![0u8; 1024];
let n = fs.read_file(&entry, 0, &mut buf)?;

// Read at offset
let n = fs.read_file(&entry, 1000, &mut buf)?;
```

### Write Files
```rust
// Find or create file
let mut entry = match fs.find_entry(&root, "output.txt") {
    Ok(e) => e,
    Err(_) => fs.create_file(root.cluster(), "output.txt")?,
};

// Write data
let data = b"Hello, FAT32!";
fs.write_file(&mut entry, 0, data)?;

// Write entire file
fs.write_file_all(&mut entry, b"Complete replacement")?;

// Truncate file
fs.truncate_file(&mut entry, 100)?;
```

### Create/Delete Files
```rust
// Create file
let file = fs.create_file(root.cluster(), "newfile.txt")?;

// Create directory
let dir = fs.create_directory(root.cluster(), "newdir")?;

// Delete file
fs.delete_file(root.cluster(), "oldfile.txt")?;

// Delete directory (must be empty)
fs.delete_directory(root.cluster(), "olddir")?;
```

### Filesystem Statistics
```rust
let stats = fs.statfs()?;
println!("Total space: {} bytes", stats.total_bytes);
println!("Free space: {} bytes", stats.free_bytes);
println!("Cluster size: {} bytes", stats.cluster_size);
println!("Clusters: {} total, {} free",
    stats.total_clusters, stats.free_clusters);
```

### Cluster Operations (Advanced)
```rust
// Get cluster chain
let chain = fs.get_cluster_chain(start_cluster)?;

// Allocate clusters
let new_chain = fs.allocate_cluster_chain(10)?; // 10 clusters

// Free cluster chain
fs.free_cluster_chain(start_cluster)?;

// Read cluster chain
let data = fs.read_cluster_chain(start_cluster, Some(max_size))?;
```

---

## FUSE - Userspace Filesystems

### Setup FUSE Connection
```rust
let connection = FuseConnection::new();
let fs = FuseFs::new(connection);
```

### File Operations
```rust
// Lookup file
let ino = fs.lookup(parent_ino, "file.txt")?;

// Get attributes
let attr = fs.getattr(ino)?;
println!("Size: {}", attr.size);
println!("Mode: {:o}", attr.mode);

// Read file
let data = fs.read(ino, 0, 4096)?;

// Write file
let written = fs.write(ino, 0, b"Data to write")?;
```

### Directory Operations
```rust
// Read directory
let entries = fs.readdir(dir_ino)?;
for name in &entries {
    println!("{}", name);
}
```

### Implement FUSE Daemon (Userspace)
```rust
// In userspace daemon
let connection = get_kernel_connection();

loop {
    // Get request from kernel
    if let Some((unique, request)) = connection.get_request() {
        // Process request
        let response = process_fuse_request(&request);

        // Send response back to kernel
        connection.deliver_response(unique, response);
    }
}
```

---

## Filesystem Detection

### Auto-detect Filesystem Type
```rust
use crate::fs::compatibility::detect_fs_type;

let device = get_device();
let fs_type = detect_fs_type(&*device.lock())?;

match fs_type {
    FilesystemType::Ext4 => {
        println!("Detected ext4");
        let fs = Ext4ReadOnlyFs::mount(device)?;
    }
    FilesystemType::Fat32 => {
        println!("Detected FAT32");
        let fs = Fat32Fs::mount(device)?;
    }
    FilesystemType::Tmpfs => {
        println!("Tmpfs (in-memory)");
    }
    _ => {
        println!("Unknown filesystem");
    }
}
```

---

## Error Handling

All operations return `FsResult<T>` which is `Result<T, FsError>`.

### Common Errors
```rust
use crate::fs::FsError;

match fs.read_file(&entry, 0, &mut buf) {
    Ok(n) => println!("Read {} bytes", n),
    Err(FsError::NotFound) => println!("File not found"),
    Err(FsError::PermissionDenied) => println!("Access denied"),
    Err(FsError::InvalidData) => println!("Corrupted filesystem"),
    Err(FsError::NoSpace) => println!("Disk full"),
    Err(FsError::IsDirectory) => println!("Is a directory"),
    Err(e) => println!("Error: {:?}", e),
}
```

### Error Types
- `NotFound` - File/directory not found
- `PermissionDenied` - Insufficient permissions
- `AlreadyExists` - File already exists
- `IsDirectory` - Expected file, got directory
- `NotDirectory` - Expected directory, got file
- `DirectoryNotEmpty` - Cannot delete non-empty directory
- `InvalidData` - Corrupted filesystem data
- `NoSpace` - Out of disk space / quota exceeded
- `IoError` - Hardware I/O error
- `NotSupported` - Operation not supported

---

## Performance Tips

### tmpfs
- Fastest option for temporary data
- All operations in RAM
- Use for build artifacts, caches, /tmp

### FAT32
- Enable FAT table caching (automatic)
- Batch operations when possible
- Sequential writes are faster than random

### ext4
- Read-only prevents accidental corruption
- Use for mounting legacy Linux drives
- Directory lookups are cached

### FUSE
- Minimize kernel-userspace transitions
- Batch requests when possible
- Use async I/O for better concurrency

---

## Complete Example: USB File Manager

```rust
use crate::fs::compatibility::*;
use crate::fs::block::BlockDevice;

pub struct UsbFileManager {
    fs: Arc<Fat32Fs>,
}

impl UsbFileManager {
    pub fn new(device: Arc<Mutex<dyn BlockDevice>>) -> FsResult<Self> {
        let fs = Fat32Fs::mount(device)?;
        Ok(Self { fs })
    }

    pub fn list_root(&self) -> FsResult<Vec<String>> {
        let root = self.fs.root();
        let entries = self.fs.read_dir(&root)?;
        Ok(entries.into_iter().map(|e| e.name).collect())
    }

    pub fn read_file(&self, path: &str) -> FsResult<Vec<u8>> {
        let root = self.fs.root();
        let entry = self.fs.find_entry(&root, path)?;
        self.fs.read_file_all(&entry)
    }

    pub fn write_file(&self, path: &str, data: &[u8]) -> FsResult<()> {
        let root = self.fs.root();
        let mut entry = match self.fs.find_entry(&root, path) {
            Ok(e) => e,
            Err(_) => self.fs.create_file(root.cluster(), path)?,
        };
        self.fs.write_file_all(&mut entry, data)
    }

    pub fn delete(&self, path: &str) -> FsResult<()> {
        let root = self.fs.root();
        self.fs.delete_file(root.cluster(), path)
    }

    pub fn stats(&self) -> FsResult<Fat32Stats> {
        self.fs.statfs()
    }
}

// Usage
let device = get_usb_device();
let manager = UsbFileManager::new(device)?;

// List files
let files = manager.list_root()?;
for file in files {
    println!("{}", file);
}

// Read/write
let data = manager.read_file("README.TXT")?;
manager.write_file("output.txt", b"Hello!")?;

// Stats
let stats = manager.stats()?;
println!("Free: {} MB", stats.free_bytes / 1024 / 1024);
```

---

## Thread Safety

All filesystem implementations are thread-safe:
- Uses `Arc` and `RwLock` for shared access
- Multiple readers or single writer
- No data races
- Safe concurrent access

```rust
use std::thread;

let fs = Arc::new(TmpFs::new());

// Spawn multiple readers
let readers: Vec<_> = (0..4).map(|i| {
    let fs = Arc::clone(&fs);
    thread::spawn(move || {
        // Safe concurrent reads
        let root = fs.get_inode(1).unwrap();
        root.read().list()
    })
}).collect();

for handle in readers {
    let _ = handle.join();
}
```
