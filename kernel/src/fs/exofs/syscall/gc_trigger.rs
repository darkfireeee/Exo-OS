//! SYS_EXOFS_GC_TRIGGER (514) — déclenche un cycle GC ExoFS.

use crate::fs::exofs::gc::sweeper::GcSweeper;
use super::validation::fserr_to_errno;

/// `exofs_gc_trigger(flags) -> 0 ou errno`
pub fn sys_exofs_gc_trigger(
    flags: u64,
    _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    let _force = (flags & 1) != 0;
    match GcSweeper::run_once() {
        Ok(_)  => 0,
        Err(e) => fserr_to_errno(e),
    }
}
