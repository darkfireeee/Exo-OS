// kernel/src/process/group/mod.rs
//
// Groupes de processus et sessions POSIX (SID, PGID).

pub mod session;
pub mod pgrp;
pub mod job_control;

pub use session::{SessionId, Session, SESSION_TABLE};
pub use pgrp::{PgId, ProcessGroup, PGROUP_TABLE};
pub use job_control::{tcsetpgrp, tcgetpgrp, JobControlError};
