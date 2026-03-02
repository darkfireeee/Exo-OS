//! SYS_EXOFS_EPOCH_COMMIT (518) — commite l'epoch courant ExoFS.

use crate::fs::exofs::epoch::epoch_commit::{commit_epoch, CommitInput};
use super::validation::fserr_to_errno;

/// `exofs_epoch_commit(flags) -> epoch_id ou errno`
pub fn sys_exofs_epoch_commit(
    flags: u64,
    _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    let input = CommitInput { flags };
    match commit_epoch(input) {
        Ok(r)  => r.epoch_id.0 as i64,
        Err(e) => fserr_to_errno(e),
    }
}
