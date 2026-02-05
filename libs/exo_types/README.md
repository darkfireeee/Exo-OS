# exo_types

Type-safe system types for Exo-OS.

## Features

- **Process IDs**: Type-safe `Pid` newtype
- **File descriptors**: RAII `FileDescriptor` wrapper
- **Error codes**: Complete errno mapping (POSIX + Exo custom)
- **Time**: Monotonic and realtime `Timestamp`
- **Signals**: Type-safe `Signal` numbers
- **User/Group IDs**: `Uid` and `Gid` newtypes
- **Syscall numbers**: Type-safe syscall constants
- **Zero-copy serialization**: bytemuck support

## Architecture

```
exo_types/
├── src/
│   ├── pid.rs          # Process ID
│   ├── fd.rs           # File descriptor (RAII)
│   ├── errno.rs        # Error codes
│   ├── time.rs         # Timestamps
│   ├── signal.rs       # Signal numbers
│   ├── uid_gid.rs      # User/Group IDs
│   └── syscall.rs      # Syscall numbers
```

## Usage

### Type-safe Process IDs

```rust
use exo_types::Pid;

let pid = Pid::new(1234);
assert_eq!(pid.as_raw(), 1234);
```

### RAII File Descriptors

```rust
use exo_types::FileDescriptor;

let fd = FileDescriptor::new(3);
// Automatically closed on drop
```

### Error Handling

```rust
use exo_types::errno::{Errno, Error};

match syscall() {
    Ok(val) => handle_success(val),
    Err(Error::Errno(Errno::ENOENT)) => handle_not_found(),
    Err(e) => handle_error(e),
}
```

### Timestamps

```rust
use exo_types::Timestamp;

let now = Timestamp::now_monotonic();
let elapsed = now.elapsed();
```

## Design Principles

- **Type safety**: Prevent mixing incompatible types
- **Zero-cost**: Repr(transparent) for no runtime overhead
- **RAII**: Automatic resource cleanup
- **Serializable**: Support for serde and bytemuck

## errno Codes

Complete POSIX errno mapping plus Exo-OS custom codes:

| Code | Value | Description |
|------|-------|-------------|
| EPERM | 1 | Operation not permitted |
| ENOENT | 2 | No such file or directory |
| ... | ... | (full POSIX + custom) |

## References

- [POSIX errno](https://pubs.opengroup.org/onlinepubs/9699919799/)
