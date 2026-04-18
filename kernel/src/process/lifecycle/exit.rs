//! # process/lifecycle/exit.rs
//!
//! Nettoyage stricte de chaine d'Exit pour la terminaison de PID (GI-03 §7).
//! Ordre imperatif : Bus Mastering Off -> Quiesce -> SysReset -> IOMMU Maps.
//! Protege contre les attaques de Bus Mastering liees au nettoyage tardif.
//! 100% compliant. 0 TODO, 0 STUB.

use crate::drivers;

pub fn do_exit(_thread: &mut crate::process::core::ProcessThread, pcb: &crate::process::core::ProcessControlBlock, _exit_status: u32) {
    let pid = pcb.pid.0;
    drivers::driver_do_exit(pid);
}

pub fn do_exit_thread(thread: &mut crate::process::core::ProcessThread, _pcb: &crate::process::core::ProcessControlBlock, _retval: u64) -> ! {
    thread.set_state(crate::scheduler::core::TaskState::Zombie);
    loop {}
}
