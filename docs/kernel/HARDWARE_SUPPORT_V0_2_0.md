# ExoOS v0.2.0 Hardware Support Matrix

This file records the supported hardware boundary for v0.2.0 so that Ring1
services and audits do not infer missing driver coverage from placeholder
modules.

| Area | v0.2.0 status | Owner |
| --- | --- | --- |
| CPU architecture | x86_64 boot only | kernel `arch/x86_64` |
| AArch64 | Not a supported boot target | future `arch/aarch64` |
| Block I/O | Kernel-owned virtio-blk path | `kernel/src/fs/exofs/storage/virtio_adapter.rs` and kernel drivers |
| Ring1 virtio service | Lifecycle/status IPC only | `servers/virtio_drivers` |
| Network | Ring1 `network_server` with virtio-net/smoltcp integration | `servers/network_server` |
| Input/TTY | Ring1 services | `servers/input_server`, `servers/tty_server` |

`servers/virtio_drivers` is intentionally not a general block-I/O backend in
v0.2.0. Critical services must not block on it for boot correctness; callers that
need block storage must use the kernel ExoFS/virtio-blk path until a future
release moves virtqueue ownership into Ring1.
