# DOC4 - Process resources

## PROC-02

DMA wakeups are registered through `process/state/wakeup.rs` and wake process
threads without importing filesystem or IPC policy into the process layer.

## Resource policy

`process/resource/` owns rlimit, usage accounting, and cgroup state. Kernel boot
must initialize the process subsystem through `process::init()` so PID,
registry, reaper, OOM hooks, DMA wakeup, and root cgroup stay synchronized.
