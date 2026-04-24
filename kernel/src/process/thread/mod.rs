// kernel/src/process/thread/mod.rs

pub mod creation;
pub mod detach;
pub mod join;
pub mod local_storage;
pub mod pthread_compat;

pub use creation::{create_thread, ThreadCreateError, ThreadCreateParams};
pub use detach::thread_detach;
pub use join::{thread_join, JoinError};
pub use local_storage::{TlsBlock, TLS_REGISTRY};
pub use pthread_compat::{
    PTHREAD_CREATE, PTHREAD_DETACH, PTHREAD_EXIT, PTHREAD_JOIN, PTHREAD_MUTEX_DESTROY,
    PTHREAD_MUTEX_INIT, PTHREAD_MUTEX_LOCK, PTHREAD_MUTEX_UNLOCK, PTHREAD_SELF,
};
