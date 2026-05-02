//! Role split for the ExoFS POSIX translation layer.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslationRole {
    VfsServer,
    KernelMechanism,
    CompatRam,
    ExternalServer,
    Phase2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceClass {
    FileIo,
    Metadata,
    Namespace,
    Space,
    Sync,
    Locking,
    VectorIo,
    Polling,
    Sparse,
    Descriptor,
    PseudoFs,
    Memory,
    Process,
    Time,
    Notification,
    Phase2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceStatus {
    Implemented,
    Delegated,
    Compat,
    Phase2,
}

impl ServiceStatus {
    pub const fn counts_for_core(self) -> bool {
        matches!(
            self,
            ServiceStatus::Implemented | ServiceStatus::Delegated | ServiceStatus::Compat
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallRoute {
    NativeExofs,
    VfsServer(TranslationRole),
    KernelBridge(TranslationRole),
    CompatOnly,
    Phase2,
    Unsupported,
}
