# ExoOS — Executive Summary

**A formally verified, capability-based microkernel with hardware-enforced AI containment.**  
Independent research project · Rust · x86_64 · April 2026

---

## What is ExoOS?

ExoOS is a from-scratch microkernel designed from first principles for security and fault tolerance. It is written in Rust (`no_std`) for x86_64 bare-metal and features:

- **Dual-kernel architecture (ExoPhoenix)** — A dedicated sentinel kernel monitors the primary kernel in real time. On anomaly detection, it freezes all cores, snapshots RAM, and restores a clean execution state — without a full reboot.
- **Hardware-enforced containment (ExoShield)** — Intel CET shadow stacks, static IOMMU policy sealed at boot, temporal capability budgets with hidden deadlines. Designed to safely contain AI workloads.
- **Capability-based security** — Every resource (memory, IRQ, DMA, PCI device) is accessed through unforgeable capability tokens. No ambient authority.
- **~95% POSIX compatibility** via ExoFS Translation Layer v5 — Wine target via Linux Shim.

## Where is the project today?

Architecture v7 is complete (5 design cycles, 45 CI checks). First boot is validated on QEMU.

**Formal verification is complete: 12/12 TLA+ modules, 60 properties, ~1.2 billion states checked.**

Every critical architectural invariant has been formally verified before a single line of implementation code is written — a practice used by seL4, AWS S2N, and Microsoft's hypervisors, but rare for independent projects.

| Verified guarantees include | |
|---|---|
| Dual-kernel mutual exclusion | DMA use-after-free prevention |
| SECURITY_READY boot ordering | Capability unforgeability |
| IRQ EOI guarantees (no LAPIC freeze) | Constant-time token verification |
| IOMMU NIC exfiltration impossible | 6-vector adversarial attack resistance |

## Why does this matter?

No existing operating system was designed to contain AI workloads at the kernel level. Linux, Windows, and even seL4 lack a threat model for AI agents that can execute arbitrary code. ExoOS is the first microkernel architecture with formally verified AI containment properties.

Beyond AI: the dual-kernel fault recovery model and capability architecture represent a meaningful advance over existing open source microkernels for high-assurance deployments.

## What is needed?

Implementation funding. The architecture and formal proofs are complete. The project needs:

- 12 months of sustained development time
- Bare-metal hardware testing infrastructure
- Cloud compute for CI and continued verification

**Requested funding: €28,000** (NLnet / Sovereign Tech Fund / OpenSSF)

This covers developer time, hardware, compute, and open source release preparation. All code and TLA+ specifications will be released under Apache 2.0 / MIT.

## Project links

- Repository: https://github.com/darkfireeee/Exo-OS
- TLA+ verification corpus: `docs/Exo-OS-TLA+/`
- Architecture documentation: `docs/architecture/`

---

*ExoOS · Architecture v7 · 12 TLA+ modules · 60 verified properties · April 2026*
