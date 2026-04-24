// kernel/src/process/signal/mod.rs
//
// Module signal/ — déplacé depuis scheduler/ (RÈGLE SIGNAL-01 DOC1).
// Signaux POSIX = cycle de vie processus, pas scheduling.

pub mod default;
pub mod delivery;
pub mod handler;
pub mod mask;
pub mod queue;
pub mod tcb;

pub use default::{SigAction, SigActionKind, Signal, DEFAULT_ACTIONS};
pub use delivery::{handle_pending_signals, send_signal_to_pid, send_signal_to_tcb};
pub use handler::{restore_signal_frame, setup_signal_frame};
pub use mask::{reset_signals_on_exec, sigprocmask, SigMask, SigSet};
pub use queue::{RTSigQueue, SigQueue, SIGQUEUE_OVERFLOW};
