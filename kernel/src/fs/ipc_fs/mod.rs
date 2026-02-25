// kernel/src/fs/ipc_fs/mod.rs
//
// Shim FS → IPC : point d'accès unique pour les sockets et pipes.
//
// RÈGLE D'ARCHITECTURE (Couche 3) : seul ce module peut dépendre de crate::ipc.
// Tous les autres sous-modules de fs/ utilisent les types re-exportés ici.

pub mod pipefs;
pub mod socketfs;

pub use pipefs::{
    create_pipe, PipeInner, PipeReadOps, PipeWriteOps,
    PIPE_BUF_SIZE, PIPE_BUF, PIPE_STATS,
};

pub use socketfs::{
    UnixSocketState, UnixSocketInner, UnixSocketOps, UnixSocketRegistry,
    socketpair, UNIX_SOCK_REGISTRY, SOCK_STATS,
};
