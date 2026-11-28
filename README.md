<div align="center">

```
               ███████╗██╗  ██╗ ██████╗       ██████╗ ███████╗
               ██╔════╝╚██╗██╔╝██╔═══██╗     ██╔═══██╗██╔════╝
               █████╗   ╚███╔╝ ██║   ██║     ██║   ██║███████╗
               ██╔══╝   ██╔██╗ ██║   ██║     ██║   ██║╚════██║
               ███████╗██╔╝ ██╗╚██████╔╝     ╚██████╔╝███████║
               ╚══════╝╚═╝  ╚═╝ ╚═════╝       ╚═════╝ ╚══════╝
```

### High-Performance, Fault-Tolerant Hybrid Microkernel

[![Status](https://img.shields.io/badge/status-v0.2.0%20Strata-22c55e?style=flat-square)](.)
[![Rust](https://img.shields.io/badge/Rust-no__std%20%2B%20std%20(target__os%3Dexo)-orange?style=flat-square&logo=rust)](.)
[![Arch](https://img.shields.io/badge/target-x86__64--unknown--exo-blue?style=flat-square)](.)
[![Proofs](https://img.shields.io/badge/proofs-Coq%20%C2%B7%20TLA%2B-8b5cf6?style=flat-square)](.)
[![Crypto](https://img.shields.io/badge/crypto-XChaCha20--Poly1305-22c55e?style=flat-square)](.)
[![License](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

<br>

*"security, performance and freedom"*

</div>

<br>

# ExoOS

**A formally verified, capability-based microkernel for x86_64 bare-metal hardware.**

ExoOS is a from-scratch Rust microkernel featuring a dual-kernel fault-tolerant architecture (**ExoPhoenix**), hardware-enforced containment and Next-Gen Antivirus (**ExoShield**), a full native port of the official Rust Standard Library (`library/std` for `target_os = "exo"`), and a complete formal verification corpus of 12 TLA+ modules covering 60 safety and liveness properties.

> **Status (v0.2.0 "Strata"):** Full consolidation of the 7 core system organs, native Rust `std` integration (`x86_64-unknown-exo.json`), and a **Strate 2 isolated 60 Hz Desktop Environment** (`exo_compositor` + `exo-ui` + VirtIO-GPU 1.2 / double-buffered scanout). Verified QEMU+KVM boot from Multiboot2 through 19 supervised Ring 1 services to the live graphical desktop (English primary / French secondary UI, ExoFS `/home/user` workspace, ExoBench, live ExoNet networking, animated ACPI S5 shutdown/reboot), alongside the `exosh` shell and 35 `std`-powered `coreutils`.

---

## Desktop Environment & Graphical Interfaces

ExoOS boots into a modern, hardware-segregated **Strate 2 Desktop** (`servers/exo_compositor`, zero direct MMIO, double-buffered synchronized damage commits over `fb_server` + VirtIO-GPU) written 100% in Rust (`exo-ui`). English is the default primary language, with live runtime switching to French in Settings.

<div align="center">
  <img src="docs/screenshots/desktop_launcher.png" width="48%" alt="ExoOS Desktop & Application Launcher — Silk Harmony wallpaper, centered dock with crystal hexagon Start button, and default user profile">
  <img src="docs/screenshots/settings_personalization.png" width="48%" alt="ExoOS Settings — Personalization with Dark/Light themes, wallpapers, harmonized accent colors, and visual transitions">
  <p><em><strong>Desktop &amp; Launcher</strong> (left) with centered dock, crystal hexagon Start emblem, and default <code>user</code> profile &nbsp;·&nbsp; <strong>Settings</strong> (right) with live themes, wallpapers, 6 harmonized accents, display brightness, EN/FR locale, and security controls.</em></p>
</div>

<div align="center">
  <img src="docs/screenshots/files_editor.png" width="48%" alt="ExoOS Files Explorer (/home/user with ExoFS BLAKE3 inspector) and Text Editor side-by-side">
  <img src="docs/screenshots/monitor_exobench.png" width="48%" alt="ExoOS System Monitor & ExoBench — Global Score 90/100 Grade A+ across Kernel, IPC, Memory, ExoFS, and 2D Compositor">
  <p><em><strong>Files &amp; Text Editor</strong> (left): writable ExoFS <code>/home/user</code> folders, BLAKE3 integrity inspector, universal scrolling, mouse selection, and flicker-free <code>Ctrl+A/C/V/Z</code> &nbsp;·&nbsp; <strong>System Monitor &amp; ExoBench</strong> (right): real-time diagnostics and 5-module performance benchmark (90/100, Grade A+).</em></p>
</div>

<div align="center">
  <img src="docs/screenshots/terminal_net.png" width="48%" alt="ExoOS Terminal running live nettest (DNS cloudflare.com & 1.1.1.1 OK) and exo doctor (Health 100/100)">
  <img src="docs/screenshots/shutdown_animation.png" width="48%" alt="ExoOS 60 Hz Shutdown Animation syncing ExoFS volumes before clean ACPI S5 power-off">
  <p><em><strong>Terminal &amp; Live Networking</strong> (left): on-demand shell app (<code>user@exo:~ $</code>), external DNS/ICMP <code>nettest</code> (4/4 OK), and <code>exo doctor</code> (100/100) &nbsp;·&nbsp; <strong>Power Sequence</strong> (right): 60 Hz animated shutdown &amp; restart syncing ExoFS before clean ACPI S5 power-off.</em></p>
</div>

---

## Boot Gallery & Early VGA Framebuffer

ExoOS includes a high-fidelity graphical display manager (`boot_display` & `fb_server`) providing live visual feedback for every kernel initialization stage and userspace service handoff.

<div align="center">
  <img src="docs/avancement/qemu_boot/img1.png" width="48%" alt="Graphical VGA Framebuffer boot screen — ARCH, MEMORY, TIME, DRIVERS stage validation">
  <img src="docs/avancement/qemu_boot/img2.png" width="48%" alt="Interactive graphical console — Handoff to PID 1 and Ring 1 services">
  <p><em>VGA Framebuffer screenshots in QEMU: modular boot sequence and transition to the interactive console.</em></p>
</div>

---

## Native Rust `std` Integration (`target_os = "exo"`)

ExoOS enforces a strict privilege boundary between `#![no_std]` kernel/critical services and full Rust `std` userspace applications (detailed in [`docs/std_integration.md`](docs/std_integration.md)):

- **Ring 0 & Critical Ring 1 (`#![no_std]`, `panic = "abort"`)**: `kernel/`, `exo-boot/`, shared ABI crates (`libs/exo_types`, `libs/exo-phoenix-ssr`, `servers/syscall_abi`), and Wave 0–2 system servers (`init_server`, `ipc_router`, `memory_server`, `device_server`, `crypto_server`, hardware drivers) remain strictly `#![no_std]`.
- **Full `rust-lang/rust` `library/std` Port (`x86_64-unknown-exo.json`)**:
  - **Platform Abstraction Layer (`library/std/src/sys/pal/exo_os/`)**: Pure-Rust PAL over ExoOS Ring 0/Ring 1 syscalls (`0..=549`), `GenericTcb`/`ExoTcb` static and per-thread dynamic TLS (`%fs:0`, `PT_TLS`), 0-syscall `sigaction` (`LOCK CMPXCHG16B`) and `sigprocmask` (`AtomicU64`) via the shared `SignalTcb` page (`AT_SIGNAL_TCB = 51`), and a 100% Rust `dlmalloc` global allocator over `SYS_MMAP`/`SYS_MREMAP`/`SYS_MUNMAP` with zero C symbols.
  - **Modular `std::sys::*` Backends (`exo.rs`)**: Native support for `alloc`, `args`, `env`, `fd`, `fs` (including `flock`, `utimensat`, `lstat` symlinks, and iterative `canonicalize`), `io` (`ioctl(TIOCGWINSZ)` terminal detection), `net` (socket timeouts via `poll` + `SO_RCVTIMEO`/`SO_SNDTIMEO`, `peek`, `getsockname`/`getpeername`, `setsockopt`/`getsockopt`), `pipe`, `process` (`fork`/`execve`/`wait4` with `PATH` lookup and child capability `pledge`), `random` (`SYS_GETRANDOM` + `AT_RANDOM`), `stdio`, `sync` (real blocking kernel `SYS_FUTEX`), `thread` (real OS threads via `SYS_CLONE` with guard-paged stacks, `CLONE_SETTLS`, and `CLONE_CHILD_CLEARTID` futex join), `thread_local`, and `time`.
  - **ExoOS-Specific Extensions (`std::os::exo`)**:
    - `CapTokenExt`: Inspect, attenuate (`clone_for_child`), and delegate capability tokens (`AT_CAP_TOKEN = 52`).
    - `ExoFsExt`: Native CoW snapshots, B+Tree searches, and extended attributes on `std::fs::File`.
    - `PhoenixSafeExt`: `PhoenixSafe<T>` fault-surviving allocations (`PROT_PHOENIX_SAFE`) and `SYS_PHOENIX_CHECKPOINT`.
    - `ExoPledgeExt`: Zero-Trust process sandboxing via `pledge()` (`SYS_EXO_PLEDGE`) and `unveil()` (`SYS_EXO_UNVEIL`).
- **Migrated Userspace (`-Z build-std=core,alloc,std,panic_abort`)**: Both the interactive shell (`servers/exosh`) and all 35 `userspace/apps/coreutils` binaries compile directly against the real Rust `std`.

## Pure-Rust Wayland Stack (`wayland-rs`)

With real `std`, blocking `epoll`/`poll`, `SYS_MEMFD_CREATE`, shared physical page `mmap(MAP_SHARED)`, and `AF_UNIX` with `SCM_RIGHTS` in place, ExoOS runs the official pure-Rust `wayland-rs` ecosystem in userspace (detailed in [`docs/wayland_rs_integration.md`](docs/wayland_rs_integration.md)):

- **Modular Wayland Crates (`libs/wayland-rs/`)**: Tracked directly in the repository (`wayland-backend`, `wayland-client`, `wayland-server`, `wayland-protocols`, `wayland-cursor`, `wayland-scanner`).
- **System Compatibility (`exo-rustix`)**: Lightweight `rustix` compatibility layer enabling standard Wayland server/client socket polling, `SCM_RIGHTS` ancillary FD transfer, and `memfd` operations on `target_os = "exo"`.
- **Compositor & Client Communication**: Zero-copy shared memory pools (`wl_shm` over `SYS_MEMFD_CREATE` + `mmap(MAP_SHARED)`) and IPC over Unix domain sockets (`AF_UNIX`) using file descriptor passing (`SCM_RIGHTS`), peer-close `EPOLLHUP`/`POLLHUP` detection, and blocking `epoll_wait` event dispatching.
- **Live Verification**: `exosh` provides the `exo wayland` builtin verifying live OS multi-threading (`std::thread::spawn` + `PT_TLS` + `SYS_FUTEX`) and full compositor/client roundtrips (`wl_compositor`, `wl_shm` pixel buffer verification via `SCM_RIGHTS` + `MAP_SHARED`, `xdg_wm_base`/`xdg_surface`/`xdg_toplevel` configure/ack_configure, and surface commits).

---

## ExoOS v0.2.0 "Strata" — Consolidation Highlights

Version **v0.2.0 ("Strata")** consolidates all kernel subsystems and Ring 1 servers across 7 audited architectural organs:

| Organ | Technical Domain | Major Advances & Resolutions |
| :--- | :--- | :--- |
| **1** | **Boot Foundation, KASLR, Clock, Memory & Sentinel** | Elimination of CoW/SLUB leaks, secure KASLR initialization, cooperative BLAKE3 Sentinel integrity monitoring bounded to 256 PML4 entries. |
| **2** | **SMP, Process Lifecycle & Synchronization** | Full reclamation of kernel stacks (256 KiB) and TIDs on PCB Drop, immediate recycling of exited threads, standard `wstatus`/exit reason encoding, SMP TLB shootdown synchronization on CoW, FPU/SSE state preservation (`fxsave64`/`fxrstor64`) across signal frames. |
| **3** | **ExoFS File System & Block Cache** | Strict protection against silent eviction of dirty blocks without writeback, Zero-Trust Multi-Level Security (MLS) access control on every object open. |
| **4** | **ExoNet V5 Network Stack & Drivers** | Strict closure of DMA-0 bypass (`BYPASS_IOMMU` removed), 100ms boot timeout with automatic fallback to `loopback_net` (`127.0.0.1`), K1 V5 CapTokens and Zero-Trust socket verification, chunked `NET_INLINE_DATA_MAX` IPC transfers. |
| **5** | **IPC, Endpoint Registry & ExoCordon** | Elimination of TOCTOU races on endpoint destruction, 128-slot raw mailboxes, dynamic service registry extended to 64 DAG services. |
| **6** | **Crypto Server & ExoShield NGAV** | Elimination of false positives (`SCAN-FP-1`) in the NGAV scanner, fail-closed IPC Gate, strict per-PID TLS session isolation, Phoenix entropy reserved for PID 0. |
| **7** | **Native Rust `std`, `exosh` Shell & Coreutils** | Full `library/std` port (`target_os = "exo"`), Unix pipelines (`cmd1 \| cmd2`) via `SYS_PIPE2`/`SYS_FORK`/`SYS_DUP2`, stream redirections (`>`, `>>`, `<`, `2>`), quoting, environment variables (`$?`, `$PWD`, `$HOME`), and 35 `std`-powered Coreutils utilities. |

---

## Quick Start

### Prerequisites
Building and running ExoOS requires Linux or **WSL2 on Windows** with QEMU and the Rust nightly toolchain:

```bash
sudo apt update
sudo apt install -y build-essential qemu-system-x86 xorriso grub-pc-bin grub-common mtools llvm rsync
rustup toolchain install nightly
rustup component add rust-src rustfmt clippy llvm-tools-preview --toolchain nightly
```

### 1. Canonical Graphical Boot (VGA Framebuffer Window)
To launch ExoOS with the graphical boot screen and interactive console:

```bash
make qemu
```
*(From Windows PowerShell: `wsl -e bash -lc "cd /mnt/c/Users/xavie/Desktop/Exo-OS && make qemu"`)*

> [!TIP]
> On Windows 11 with WSLg, the `Makefile` automatically detects the X11 socket (`/tmp/.X11-unix/X0`) and exports `DISPLAY=:0`, displaying the QEMU window directly on your Windows desktop.

### 2. Headless Boot (Terminal Mode)
For a purely text-based session inside your current terminal without opening an external window:

```bash
make qemu-nographic
```
*(To exit QEMU in nographic mode: press `Ctrl + A` then `X`)*

### 3. Live Kernel & Service Log Monitoring
To inspect real-time boot markers and debug output via the QEMU `debugcon` (`0xe9`) port:

```bash
tail -f /tmp/e9k.txt
```

---

## Ring 1 Supervision Architecture (19 Synchronized Services + Shell)

In ExoOS v0.2.0, service orchestration is managed by `init_server` (PID 1) according to a strict dependency DAG verified by `check_service_order.py`:

| PID | Service / Component | Role & Responsibility |
| :---: | :--- | :--- |
| **1** | `init_server` | Root supervisor, service graph orchestration, and fault recovery |
| **2** | `ipc_router` | IPC message routing and endpoint name registry |
| **3** | `memory_server` | Ring 1 shared memory allocation and management |
| **4** | `loopback_driver` | Virtual local network interface (`127.0.0.1`) |
| **5** | `vfs_server` | Virtual File System bridged over ExoFS |
| **6** | `device_server` | Hardware device enumeration and management |
| **7** | `scheduler_server` | Ring 1 cooperative scheduling service |
| **8** | `crypto_server` | Cryptographic primitives (XChaCha20, Ed25519, TLS, BLAKE3) |
| **9** | `input_server` | Keyboard and mouse event multiplexer |
| **10** | `fb_server` | Strate 1 framebuffer MMIO guardian, `SurfaceGrant` scanout & emergency console |
| **11** | `virtio_drivers` | VirtIO Control Plane & VirtIO-GPU scanout/flush (endpoint 13) |
| **12** | `e1000_driver` | Intel Gigabit Ethernet network card driver |
| **13** | `virtio_net_driver` | High-performance VirtIO PCI network driver |
| **14** | `network_server` | ExoNet V5 network stack (IP, TCP, UDP, DNS, Unix sockets) |
| **15** | `audio_server` | Ring 1 Wave 4 Intel HDA audio server |
| **16** | `usb_server` | Ring 1 Wave 4 xHCI USB host controller server |
| **17** | `tty_server` | Virtual terminal manager and ANSI emulation |
| **18** | `ps2_driver` | PS/2 keyboard & mouse driver (i8042 Scan Code Set 2) |
| **19** | `exo_shield` | NGAV module, AI containment, anomaly detection & forensics |
| **20** | `exo_compositor` | Strate 2 isolated Wayland compositor (`/run/wayland-0`, zero MMIO, Rust `std`) |
| **21** | `exosh` | Interactive user console and root shell (compiled with Rust `std`) |

---

## User Shell Capabilities (`exosh`)

The ExoOS shell provides a complete interactive environment for exploring the system and manipulating files on ExoFS:

### Commands and Utilities
- **Navigation & Files**: `pwd`, `cd`, `cd -`, `ls` (`-l`, `-a`, `-la`, `-lah`), `mkdir`, `rmdir`, `touch`, `cat`, `echo`, `cp`, `mv`, `rm` (`-r`, `-rf`), `dd`, `tree`, `stat`, `sync`, `basename`, `dirname`.
- **Filters & Pipes**: Full Unix pipelines (`cmd1 | cmd2 | cmd3`), `grep` (`-i`, `-n`, `-v`), `wc`, and stream redirections (`>`, `>>`, `<`, `2>`).
- **Processes, Capabilities & Diagnostics**: `exo` (`exo cap`), `top`, `ps`, `kill`, `kill -9`, `sleep`, `uptime`, `meminfo`, `ipc-stat`, `syscall-stat`, `uname`, `whoami`, `calendar`, `clear`.
- **Line Editing**: Command history (`Up` / `Down` arrows), cursor movement (`Left` / `Right`), tab completion, `Ctrl+L` (clear screen), `Ctrl+C` (interrupt).

### Interactive Session Example
```text
exosh:/$ echo "ExoOS v0.2.0 Strata" > /tmp/strata.txt
exosh:/$ cat /tmp/strata.txt | grep -i strata
ExoOS v0.2.0 Strata
exosh:/$ ls -lah /bin | grep grep
-rwxr-xr-x 1 root root 84K Oct 18 16:30 grep
exosh:/$ echo "Exit status: $?"
Exit status: 0
exosh:/$ exo cap
```

---

## Formal Verification (TLA+)

The global architecture is formally specified and verified across 12 TLA+ modules covering 60 safety and liveness properties (starvation freedom, dual-kernel exclusivity, capability token unforgeability, IOMMU integrity, context switch atomicity).

| TLA+ Module | Verified States | Status |
| :--- | :---: | :---: |
| 1 · ExoPhoenix Dual-Kernel Handoff | 178,992 states | ✅ Verified |
| 2 · SMP Boot Sequence (18-step) | 481 states | ✅ Verified |
| 3 · IRQ Routing & Atomic Invariants | 524,288 states | ✅ Verified |
| 4 · IOMMU Fault Queue (CAS-based) | 34,790 states | ✅ Verified |
| 5 · PCI Claim & do_exit() 7-step | 37,133 states | ✅ Verified |
| 6 · Context Switch Atomicity | 135,117 states | ✅ Verified |
| 7 · ExoFS Crash Consistency | 5,128 states | ✅ Verified |
| 8+9 · ExoShield + CapTokens | 107,584 states | ✅ Verified |
| 10 · Process Death & fd_table restore | 342 states | ✅ Verified |
| 11 · Memory Ordering (Release/Acquire) | 184 states | ✅ Verified |
| 12 · Adversarial Attack Surface | 1,495 states | ✅ Verified |
| **Full Composition (Monte Carlo)** | **565,076,967 states** | **✅ Verified** |

---

## Repository Structure & Modularity

The ExoOS codebase enforces strict modular segregation where every major subsystem is decomposed into single-responsibility modules without god files (> 900 lines):

```text
Exo-OS/
├── bootloader/                 # GRUB 2 and Multiboot2 configuration
├── x86_64-unknown-exo.json     # Custom Rust target specification for target_os = "exo"
├── library/std/                # Native port of rust-lang/rust library/std for ExoOS
│   ├── src/sys/pal/exo_os/     # Low-level PAL (syscalls, TCB/SignalTcb runtime, dlmalloc, futex)
│   ├── src/sys/*/exo.rs        # Per-subsystem std::sys backends (fs, net, process, thread, fd...)
│   └── src/os/exo/             # Public std::os::exo traits (CapTokenExt, ExoFsExt, PhoenixSafeExt, ExoPledgeExt)
├── kernel/                     # Ring 0 — Hybrid Rust #![no_std] microkernel
│   ├── src/arch/x86_64/        # BSP/AP boot, GDT/IDT, KASLR, Page Tables, HPET/TSC, early Framebuffer
│   ├── src/memory/             # Modular Buddy allocator, SLUB, CoW, VMA, KPTI
│   ├── src/process/            # Modular PCB, registry, 0-syscall SignalTcb & signal delivery, lifecycle
│   ├── src/scheduler/          # SMP scheduler, KTCB, context switch, timers
│   ├── src/security/           # ExoShield, ExoSeal, ExoCage (CET), Zero-Trust Pledge, ExoKairos
│   ├── src/ipc/                # SpscRing, CapTokens, Zero-Copy SHM, raw mailboxes
│   ├── src/fs/                 # ExoFS file system, block cache, ELF64 loader (PT_TLS, SignalTcb, auxv)
│   ├── src/syscall/            # O(1) dispatch table, fs_bridge, net_bridge, ipc_bridge
│   └── src/exophoenix/         # Dual-Kernel Sentinel & Stage0 fault recovery
├── servers/                    # Ring 1 supervised servers and interactive shell
│   ├── init_server/            # PID 1 — Root supervisor and DAG service bootstrap
│   ├── ipc_router/             # PID 2 — Endpoint directory and IPC routing
│   ├── memory_server/          # PID 3 — Ring 1 memory manager
│   ├── vfs_server/             # PID 5 — VFS server
│   ├── device_server/          # PID 6 — Hardware device server
│   ├── scheduler_server/       # PID 7 — Ring 1 scheduler
│   ├── crypto_server/          # PID 8 — Cryptographic server and TLS isolation
│   ├── input_server/           # PID 9 — Input multiplexer
│   ├── fb_server/              # PID 10 — Graphical Framebuffer server
│   ├── network_server/         # PID 14 — ExoNet V5 network stack (NetGrant ABI, TCP/UDP/IP)
│   ├── audio_server/           # PID 15 — Intel HDA audio server
│   ├── usb_server/             # PID 16 — xHCI USB host controller server
│   ├── tty_server/             # PID 17 — TTY server and console emulation
│   ├── exo_shield/             # PID 19 — NGAV, AI containment, anomaly detection & forensics
│   ├── exo_compositor/         # PID 20 — Strate 2 isolated 60 Hz Desktop & Wayland compositor (Rust std)
│   └── exosh/                  # PID 21 — Interactive shell compiled with Rust std
├── drivers/                    # Ring 1 drivers (PS/2, VirtIO-Net, e1000, Loopback, VirtIO-GPU, VirtIO-Blk)
├── libs/                       # Runtime & UI crates (exo-ui, exo-abi, exo-wire, exo-srv-rt, wayland-rs...)
├── userspace/apps/coreutils/   # 35 standard utilities compiled with Rust std (cat, grep, ls, ps, top, exo...)
└── docs/                       # Technical documentation, screenshots/, std_integration.md & TLA+ proofs
```

---

*ExoOS — v0.2.0 "Strata" · November 2026*  
*12 TLA+ modules · 60 properties · Over 565 million formally verified states*
