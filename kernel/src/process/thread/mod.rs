// kernel/src/process/thread/mod.rs

pub mod creation;
pub mod join;
pub mod detach;
pub mod local_storage;
pub mod pthread_compat;

pub use creation::{create_thread, ThreadCreateParams, ThreadCreateError};
pub use join::{thread_join, JoinError};
pub use detach::thread_detach;
pub use local_storage::{TlsBlock, TLS_REGISTRY};
pub use pthread_compat::{
    PTHREAD_CREATE, PTHREAD_JOIN, PTHREAD_DETACH, PTHREAD_SELF,
    PTHREAD_EXIT, PTHREAD_MUTEX_INIT, PTHREAD_MUTEX_LOCK,
    PTHREAD_MUTEX_UNLOCK, PTHREAD_MUTEX_DESTROY,
};
