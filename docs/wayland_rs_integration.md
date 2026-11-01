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

1. **`epoll` Subsystem** (`kernel/src/syscall/fs_bridge/poll.rs`):
   - Registered watches with `EPOLL_CTL_ADD`, `EPOLL_CTL_MOD`, `EPOLL_CTL_DEL`.
   - File descriptor readiness polling (`EPOLLIN`, `EPOLLOUT`, `EPOLLERR`, `EPOLLHUP`).

2. **Unix Domain Sockets & `SCM_RIGHTS`** (`kernel/src/syscall/fs_bridge/special.rs`):
   - `AF_UNIX` stream socket pairs and named filesystem sockets.
   - Ancillary message parsing for `SCM_RIGHTS`: passing open file descriptors between processes across Wayland client and compositor boundaries.

## Verification

- **Compilation**: Clean compilation of all `wayland-rs` crates under `x86_64-unknown-exo.json`.
- **Live Roundtrip**: `exosh` runs live client-server interaction (`wl_compositor`, `wl_shm`, `xdg_wm_base`, surface creation).
- **QEMU Boot**: ISO image boots successfully to userspace `exosh:/$` interactive shell with GUI display and Wayland terminal window.
