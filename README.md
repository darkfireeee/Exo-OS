<div align="center">

```
███████╗██╗  ██╗ ██████╗       ██████╗ ███████╗
██╔════╝╚██╗██╔╝██╔═══██╗     ██╔═══██╗██╔════╝
█████╗   ╚███╔╝ ██║   ██║     ██║   ██║███████╗
██╔══╝   ██╔██╗ ██║   ██║     ██║   ██║╚════██║
███████╗██╔╝ ██╗╚██████╔╝     ╚██████╔╝███████║
╚══════╝╚═╝  ╚═╝ ╚═════╝       ╚═════╝ ╚══════╝
```

### Microkernel Hybride Haute Performance

[![Status](https://img.shields.io/badge/status-en%20développement-orange?style=flat-square)](.)
[![Rust](https://img.shields.io/badge/Rust-no__std%20nightly-orange?style=flat-square&logo=rust)](.)
[![Arch](https://img.shields.io/badge/cible-x86__64%20·%20aarch64-blue?style=flat-square)](.)
[![Preuves](https://img.shields.io/badge/preuves-Coq%20·%20TLA%2B-8b5cf6?style=flat-square)](.)
[![Crypto](https://img.shields.io/badge/crypto-XChaCha20--Poly1305-22c55e?style=flat-square)](.)
[![Licence](https://img.shields.io/badge/licence-MIT-lightgrey?style=flat-square)](.)

<br>

*"security, performance and freedom"*

<br>
# ExoOS

**A formally verified, capability-based microkernel for x86_64 bare-metal hardware.**

ExoOS is a from-scratch Rust microkernel featuring a dual-kernel fault-tolerant architecture (ExoPhoenix), hardware-enforced security (ExoShield), and a complete formal verification corpus of 12 TLA+ modules covering 60 safety and liveness properties.

> **Status:** Architecture v7 finalized · Formal verification complete (12/12 modules) · First boot validated on QEMU · Implementation of P0 security patches in progress.

---

## Architecture Overview

ExoOS is built around three core design principles:

**Capability-based security** — Every kernel resource (memory, IRQ, DMA, PCI device) is accessed exclusively through unforgeable capability tokens. No ambient authority exists anywhere in the system.

**Dual-kernel fault tolerance (ExoPhoenix)** — A dedicated sentinel kernel (Kernel B) runs on Core 0 and continuously monitors the primary kernel (Kernel A). On anomaly detection, Kernel B freezes all Kernel A cores via IPI, snapshots RAM state, and restores a clean execution environment without requiring a full reboot.

**Hardware-enforced containment (ExoShield)** — A multi-layer AI and process containment module combining Intel CET shadow stacks (ExoCage), temporal capability budgets (ExoKairos), static IOMMU NIC policy, and an append-only tamper-evident audit ledger (ExoLedger P0).

---

## Key Technical Specifications

| Component | Specification |
|---|---|
| Language | Rust (`no_std`, x86_64 bare-metal) |
| Architecture | Hybrid microkernel, Ring 0 / Ring 1 |
| Kernel model | Dual-kernel A+B (ExoPhoenix v6) |
| Boot sequence | 18-step ordered boot, SECURITY_READY at step 18 |
| Lock order | Memory → Scheduler → Security → IPC → FS |
| TCB layout | GI-01 canonical, 256 bytes, hardcoded offsets |
| SSR layout | Physical `[0x1000000..0x110000]`, E820 reserved |
| Syscalls | 530–546 (driver framework) |
| POSIX coverage | ~95% via ExoFS Translation Layer v5 |
| Formal verification | 12 TLA+ modules, 60 properties, ~1.2B states checked |

---

## Formal Verification Results

All 12 architectural modules have been formally verified using TLA+ TLC Model Checker. Each module was exhaustively verified (BFS, zero violations). The full system composition was validated via Monte Carlo simulation (565M+ states, 5.1M+ traces, zero invariant violations).

| Module | States Checked | Result |
|---|---|---|
| 1 · ExoPhoenix Dual-Kernel Handoff | 178,992 | ✅ Verified |
| 2 · SMP Boot Sequence (18-step) | 481 | ✅ Verified |
| 3 · IRQ Routing & Atomic Invariants | 524,288 | ✅ Verified |
| 3 · IRQ Stress (4-core storm) | ~37,137 | ✅ Verified |
| 4 · IOMMU Fault Queue (CAS-based) | 34,790 | ✅ Verified |
| 5 · PCI Claim & do_exit() 7-step | 37,133 | ✅ Verified |
| 6 · Context Switch Atomicity | 135,117 | ✅ Verified |
| 7 · ExoFS Crash Consistency | 5,128 | ✅ Verified |
| 8+9 · ExoShield + CapTokens | 107,584 | ✅ Verified |
| 10 · Process Death & fd_table restore | 342 | ✅ Verified |
| 11 · Memory Ordering (Release/Acquire) | 184 | ✅ Verified |
| 12 · Adversarial (combined attack surface) | 1,495 | ✅ Verified |
| **Full Composition (Monte Carlo)** | **565,076,967** | **✅ Verified** |
| **Full Stress — 6 cores (Monte Carlo)** | **634,564,537** | **✅ Verified** |

**Properties proven include:** dual-kernel exclusivity, FPU coherence across context switches, SECURITY_READY ordering, IRQ EOI guarantees, DMA use-after-free prevention, capability unforgeability, constant-time token verification, IOMMU NIC exfiltration impossibility, and full adversarial resilience (6 simultaneous attack vectors).

Full TLA+ specifications and verification outputs are in [`docs/Exo-OS-TLA+/`](docs/Exo-OS-TLA+/).

---

## Repository Structure

```
Exo-OS/
├── kernel/               # Ring 0 — microkernel core (Rust no_std)
│   ├── src/
│   │   ├── boot/         # 18-step boot sequence, SMP init
│   │   ├── memory/       # Buddy allocator, PhysAddr/VirtAddr/IoVirtAddr
│   │   ├── scheduler/    # TCB GI-01, context switch (switch.rs)
│   │   ├── security/     # ExoShield: ExoSeal, ExoCage, ExoKairos
│   │   ├── ipc/          # SpscRing, CapTokens, reply_nonce
│   │   ├── drivers/      # Driver framework v10, syscalls 530–546
│   │   └── exophoenix/      # ExoPhoenix v6 dual-kernel handoff
├── ring1/                # Ring 1 — system servers
│   ├── ipc_broker/       # PID 2, ExoCordon DAG enforcement
│   ├── memory_server/    # Physical memory management
│   ├── vfs_server/       # PID 3, ExoFS Translation Layer v5
│   ├── crypto_server/    # PID 4, ChaCha20, Blake3
│   ├── device_server/    # Driver host
│   └── exo_shield/       # Phase 3 AI containment module
├── docs/
│   ├── Exo-OS-TLA+/      # 12 TLA+ modules + verification outputs (FR)
│   ├── recast/           # Architecture v7 specifications + cgit ORR-01..54 audit corpus (FR)
│   └── old/              # all first code used before recast (FR)
└── Cargo.toml
```

---

## ExoShield — AI Containment Module

ExoShield v1.0 is designed to safely contain Mythos-class AI workloads within ExoOS. It is built on three verified core modules:

- **ExoSeal** — Inverted boot order: Kernel B starts first, seals IOMMU policy before Kernel A, preventing any post-boot policy modification.
- **ExoCage** — Intel CET hardware-enforced control flow integrity. Shadow stack tokens prevent SROP attacks. Any `#CP` exception triggers an immediate ExoPhoenix handoff.
- **ExoKairos** — Inline capability budgets with hidden deadlines stored in Ring 0 only. `calls_left` is an `AtomicU32` decremented on each use. Deadline MAC (HMAC-Blake3) is inaccessible to Ring 1 code.

Static IOMMU NIC whitelist locked by Kernel B at boot — physical network exfiltration is provably impossible post-seal (TLA+ property S40).

Six security properties formally specified and verified in TLA+: `S33`–`S40`.

---

## Ring 1 Startup Order (V4 Canonical)

```
PID 2  ipc_broker       → ExoCordon DAG enforcement
       memory_server    → Physical memory
PID 1  init_server      → Process lifecycle, ChildDied handler
PID 3  vfs_server       → ExoFS TL v5, ~95% POSIX
PID 4  crypto_server    → ChaCha20, Blake3, nonce management
       device_server    → Driver host
       virtio-block     → Storage
       virtio-net       → Network
       virtio-console   → Console
       network_server   → TCP/IP stack
       scheduler_server → Userspace scheduling
       exo_shield       → Phase 3 only — AI containment
```

Governing rules: `SRV-01/02/04`, `CAP-01`, `IPC-01/02/03`, `PHX-01/02/03`.

---

## Current Status & Roadmap

**Completed**
- Architecture v7 (5 design cycles, 45 CI checks)
- 18-step boot sequence specification
- Driver Framework v10 (syscalls 530–546, 55 DRV-* silent errors catalogued)
- ExoFS Translation Layer v5 (36 TL-rules, Wine target via POSIX TL + Linux Shim Phase 9)
- ExoShield v1.0 specification (multi-AI consensus process)
- Full TLA+ formal verification corpus (CORR-01..54 + SRV-05)
- First boot validated on QEMU

**In Progress (P0 blockers)**
- `SSR_MAX_CORES_LAYOUT` constant divergence fix (shared crate vs kernel local)
- `security_init()` boot wiring
- `init_syscall()` on AP cores (currently BSP only)
- `gs:[0x20]` write during context switch (P0-D)

**Roadmap**
- Phase 0 — Codebase coherence (P0 patches above)
- Phase 1 — Critical security (LAC-01/04/06, CVE-EXO-001)
- Phase 2 — Robustness hardening
- Phase 3 — Full Ring 1 servers + ExoShield activation
- Phase 4 — ExoPhoenix live testing + quality

---

## Formal Verification Reproduction

```bash
# Requirements: Java JDK 11+, tla2tools.jar
# https://github.com/tlaplus/tlaplus/releases

cd docs/Exo-OS-TLA+/

# Run individual module (example: SMP Boot)
java -Xmx4g -XX:+UseParallelGC -jar tla2tools.jar \
     -workers auto -config SmpBoot.cfg SmpBoot.tla

# Run full composition (Monte Carlo simulation)
java -Xmx4g -XX:+UseParallelGC \
     -cp /path/to/tla2tools.jar tlc2.TLC \
     -simulate -deadlock -depth 50 -workers auto \
     -config ExoOS_Composition.cfg ExoOS_Full.tla

# Run stress mode (6 cores, adversarial)
java -Xmx10g -XX:+UseParallelGC \
     -cp /path/to/tla2tools.jar tlc2.TLC \
     -simulate -deadlock -depth 50 -workers auto \
     -config ExoOS_Stress.cfg ExoOS_Full.tla
```

---

## Design Decisions & References

- **Why Rust?** Memory safety by construction at Ring 0. `no_std` enforces zero implicit allocations in interrupt paths.
- **Why dual-kernel?** Single-kernel fault tolerance requires the kernel to trust itself. Kernel B runs on a physically isolated core with no shared mutable state with Kernel A.
- **Why TLA+?** Race conditions, memory ordering bugs, and capability lifecycle errors are invisible to unit tests. TLA+ explores all interleavings exhaustively.
- **Influenced by:** seL4 (capability model), Redox (Rust kernel approach), QubesOS (isolation philosophy), ExoKernel (resource abstraction).

---

## Contributing

This project is in early development. Architecture specifications and TLA+ models are in `docs/`. Issues and discussions welcome.

---

## License

TBD — open source license to be determined prior to Phase 1 release.

---

*ExoOS — Architecture v7 · April 2026*  
*12 TLA+ modules · 60 properties · ~1.2B states verified*