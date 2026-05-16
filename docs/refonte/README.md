# ExoOS refonte reference set

This directory turns the historical `DOC1`/`DOC2`/`DOC3` source comments into
repository-local references.  The canonical long-form architecture remains in
`docs/recast/`; these files are the compact engineering contracts used by the
kernel comments.

- `DOC1.md`: process, signal, syscall return, capability ownership.
- `DOC2.md`: memory bootstrap, physmap, TLB shootdown, protection ordering.
- `DOC3.md`: scheduler, IPC fast path, FPU/lazy state, preemption contracts.
- `DOC4.md`: process resources, cgroup, rlimit, wakeup hooks.
- `DOC5.md`: permitted cross-layer scheduler/IPC hooks.
- `DOC7.md`: capability table, revocation, delegation.
- `regle_bonus.md`: unsafe comments, no-allocation zones, lock ordering.
