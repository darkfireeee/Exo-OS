// kernel/src/process/group/mod.rs
//
// Groupes de processus et sessions POSIX (SID, PGID).

pub mod job_control;
pub mod pgrp;
pub mod session;

pub use job_control::{tcgetpgrp, tcsetpgrp, JobControlError};
pub use pgrp::{PgId, ProcessGroup, PGROUP_TABLE};
pub use session::{Session, SessionId, SESSION_TABLE};
