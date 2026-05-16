# Bonus engineering rules

## Unsafe contract

Every non-trivial `unsafe` block must state the invariant that makes it valid:
pointer provenance, table ownership, interrupt state, CPU-locality, or lifetime.

## No-allocation zones

Scheduler core, interrupt handlers, TLB shootdown paths, and low-level IPC ring
operations are no-allocation zones unless a file explicitly documents a bounded
preallocated pool.

## Lock ordering

Global lock order is:

Memory -> Scheduler -> Security -> IPC -> FS

Code must not acquire a lock from an earlier layer while holding a later-layer
lock.
