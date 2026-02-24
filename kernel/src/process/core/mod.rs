// kernel/src/process/core/mod.rs
//
// Sous-module core/ du module process/ — types fondamentaux.
// Ne dépend que de memory/ et scheduler/.

pub mod pid;
pub mod pcb;
pub mod tcb;
pub mod registry;

pub use pid::{Pid, Tid, PidAllocator, PID_ALLOCATOR, TID_ALLOCATOR};
pub use pcb::{ProcessControlBlock, ProcessState, ProcessFlags, OpenFileTable};
pub use tcb::{ProcessThread, ThreadAddress, KSTACK_SIZE};
pub use registry::{PROCESS_REGISTRY, ProcessRegistry};
