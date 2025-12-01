# POSIX-X Integration Test Suite

This directory contains integration tests for the POSIX-X compatibility layer.

## Tests

### hello_musl.c

Simple "Hello World" program that tests:

- `printf()` via musl libc
- `write()` syscall to stdout
- Basic program startup (argc/argv)

**Build:**

```bash
pwsh tests/build_hello_musl.ps1
```

**Expected Output** (when run in Exo-OS):

```
Hello from musl on Exo-OS!
This is POSIX-X in action!
Direct write() syscall test
argc = 1
argv[0] = /hello_musl.elf
```

## Running Tests

Tests require Exo-OS to have:

1. ELF loader
2. Process/thread support
3. POSIX-X syscall layer active

Load ELF files as userspace processes via the kernel's loader.

## Future Tests

- `file_io_test.c` - open/read/write/close
- `fork_test.c` - process creation
- `signal_test.c` - signal handling
- `pipe_test.c` - IPC via pipes
