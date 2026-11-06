// kernel/src/process/core/mod.rs
//
// Core submodule of the process/ module — fundamental types.
// Only depends on memory/ and scheduler/.

pub mod pcb;
pub mod pid;
pub mod registry;
pub mod tcb;

pub use pcb::{OpenFileTable, ProcessControlBlock, ProcessFlags, ProcessState};
pub use pid::{Pid, PidAllocator, Tid, PID_ALLOCATOR, TID_ALLOCATOR};
pub use registry::{ProcessRegistry, PROCESS_REGISTRY};
pub use tcb::{ProcessThread, ThreadAddress, KSTACK_SIZE};
