// kernel/src/process/signal/mod.rs
//
// Module signal/ — déplacé depuis scheduler/ (RÈGLE SIGNAL-01 DOC1).
// Signaux POSIX = cycle de vie processus, pas scheduling.

pub mod default;
pub mod delivery;
pub mod handler;
pub mod mask;
pub mod queue;

pub use default::{Signal, SigAction, SigActionKind, DEFAULT_ACTIONS};
pub use delivery::{handle_pending_signals, send_signal_to_pid, send_signal_to_tcb};
pub use handler::{setup_signal_frame, restore_signal_frame};
pub use mask::{SigMask, SigSet, sigprocmask, reset_signals_on_exec};
pub use queue::{SigQueue, RTSigQueue, SIGQUEUE_OVERFLOW};
