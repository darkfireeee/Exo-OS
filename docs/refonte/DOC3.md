# DOC3 - Scheduler, IPC, FPU

## SCHED-01

Architecture C ABI bridges may call scheduler entry points only through the
documented `arch/x86_64/sched_iface.rs` surface.

## SCHED-03

Futex ownership belongs to `kernel/src/memory/utils/futex_table.rs`; scheduler
code may block and wake through registered wait queues, but must not own futex
hashing or user-address lookup.

## FPU-01

Lazy FPU policy belongs to `scheduler/fpu/lazy.rs`. Architecture code saves and
restores hardware state; scheduler code decides when lazy state becomes active.

## FPU-02

The `#NM` handler delegates to scheduler FPU policy. It must not duplicate lazy
FPU bookkeeping in exception code.

## IPI-01

Scheduler IPIs set reschedule state and wake CPUs. They must not directly run
the scheduler while holding architecture interrupt-controller locks.
