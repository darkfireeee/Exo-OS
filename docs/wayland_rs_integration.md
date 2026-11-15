# Exo-OS: Wayland-rs Integration Guide

## Overview

Following the migration to the real Rust standard library (`target_os = "exo"`), the official pure-Rust Wayland implementation (`wayland-rs`) has been integrated into the Exo-OS workspace.

## Architecture

```
                  +----------------------------------------------+
                  |         Wayland Clients & Servers            |
                  |   (wayland-client, wayland-server, exosh)    |
                  +----------------------------------------------+
                                         |
                                         v
                  +----------------------------------------------+
                  |               wayland-backend                |
                  |         (RS backend, pure Rust wire)         |
                  +----------------------------------------------+
                                         |
                                         v
                  +----------------------------------------------+
                  |                 exo-rustix                   |
                  |     (rustix v1.0.2 patch for target_os="exo")|
                  +----------------------------------------------+
                                         |
                                         v
                  +----------------------------------------------+
                  |              Rust library/std                |
                  |       (std::os::unix, std::os::exo::unix)    |
                  +----------------------------------------------+
                                         |
                                         v
+--------------------------------------------------------------------------------+
|                             Exo-OS Kernel (Ring 0)                             |
|  - epoll (SYS_EPOLL_CREATE1, SYS_EPOLL_CTL, SYS_EPOLL_WAIT)                     |
|  - AF_UNIX sockets (SYS_SOCKETPAIR, SYS_BIND, SYS_CONNECT, SYS_ACCEPT)         |
|  - SCM_RIGHTS FD transfer (SYS_SENDMSG, SYS_RECVMSG)                           |
+--------------------------------------------------------------------------------+
```

## Workspace Organization

- All crates reside under `libs/wayland-rs/`:
  - `libs/wayland-rs/wayland-backend`: Wire protocol and event loop abstractions
  - `libs/wayland-rs/wayland-client`: Type-safe client library
  - `libs/wayland-rs/wayland-server`: Wayland compositor/server library
  - `libs/wayland-rs/wayland-protocols`: Standard Wayland protocols (XDG shell, etc.)
  - `libs/wayland-rs/wayland-cursor`: Cursor theme parser and buffer loader
  - `libs/wayland-rs/wayland-scanner`: XML protocol code generator
  - `libs/wayland-rs/exo-rustix`: Dedicated `rustix` compatibility layer for `target_os = "exo"`

## Kernel Enhancements

1. **`epoll` & `poll` Subsystem** (`kernel/src/syscall/fs_bridge/poll.rs`):
   - Registered watches with `EPOLL_CTL_ADD`, `EPOLL_CTL_MOD`, `EPOLL_CTL_DEL`.
   - File descriptor readiness polling (`EPOLLIN`, `EPOLLOUT`, `EPOLLERR`, `EPOLLHUP`, `POLLHUP`) with real blocking sleep/timeout (`timeout_ms > 0` or `-1`) and peer-close detection.

2. **Unix Domain Sockets & `SCM_RIGHTS`** (`kernel/src/syscall/fs_bridge/special.rs`):
   - `AF_UNIX` stream socket pairs and named filesystem sockets with spinlock-protected data and ancillary FD queues (`UnixSocketPairState`), reference-counted endpoint tracking (`a_refs`/`b_refs`), EOF/`EPIPE` on peer close, and proper cleanup on `close`, `fork`, and process `exit`.
   - Ancillary message parsing and serialization for `SCM_RIGHTS` (`SOL_SOCKET = 1`, `SCM_RIGHTS = 1`): passing open file descriptors (both ExoFS objects and special FDs) between processes across Wayland client and compositor boundaries.

3. **Shared Memory Pools (`wl_shm`)** (`kernel/src/syscall/handlers/memory.rs`, `kernel/src/syscall/fs_bridge/file_ops.rs`):
   - `SYS_MEMFD_CREATE` (`319`) creates anonymous ExoFS-backed memory files in `/tmp/.memfd_*`.
   - `SYS_MMAP` (`9`) with `MAP_SHARED` maps physical frames from the shared `EXOFS_SHARED_MAPPING_TABLE` (retaining physical frame references across `munmap` and `fork`), enabling true zero-copy shared pixel buffers between Wayland clients and the compositor.

## Verification

- **Compilation**: Clean compilation of all `wayland-rs` crates under `x86_64-unknown-exo.json` (tracked directly in Git under `libs/wayland-rs/`).
- **Live Roundtrip**: `exosh` (`exo wayland`) runs both OS multi-threading verification (`std::thread::spawn` + `PT_TLS` + `SYS_FUTEX`) and a full Wayland client-compositor roundtrip (`SYS_MEMFD_CREATE` + `mmap(MAP_SHARED)` + `wl_shm.create_pool` via `SCM_RIGHTS` + `wl_shm_pool.create_buffer` + `wl_surface.attach`/`damage`/`commit` + `xdg_wm_base`/`xdg_surface`/`xdg_toplevel` configure/ack_configure), verifying that the ARGB8888 pixel (`0xFF336699`) written by the client is read back by the compositor from its `SCM_RIGHTS`-received shared memory FD.
- **QEMU Boot**: ISO image boots to userspace `exosh:/$` interactive shell with GUI display and Wayland terminal window.
