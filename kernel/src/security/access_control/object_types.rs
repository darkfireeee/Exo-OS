// kernel/src/security/capability/access_control/object_types.rs
//
// ObjectKind — catalog of protected object types (v6)
//
// Each variant documents expected rights for typical operations.
// Used by checker::check_access() for auditing and error messages.

use crate::security::capability::Rights;

/// Type of object protected by the capability system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectKind {
    /// IPC channel (named or anonymous).
    /// Rights: `READ` (recv) | `WRITE` (send)
    IpcChannel = 0,
    /// IPC endpoint (service connection point).
    /// Rights: `EXEC` (connect) | `WRITE` (publish)
    IpcEndpoint = 1,
    /// Shared memory region.
    /// Rights: `READ` | `WRITE`
    ShmRegion = 2,
    /// Regular file.
    /// Rights: `READ` | `WRITE` | `EXEC`
    File = 3,
    /// Directory.
    /// Rights: `READ` (list) | `WRITE` (create/delete)
    Directory = 4,
    /// Process or thread.
    /// Rights: `WRITE` (signal) | `EXEC` (ptrace/debug)
    Process = 5,
    /// Cryptographic key.
    /// Rights: `EXEC` (key usage)
    CryptoKey = 6,
}

impl ObjectKind {
    /// Suggested minimal rights for typical operations on this type.
    /// Used in audit error messages; is NOT a whitelist.
    pub const fn typical_rights(self) -> Rights {
        match self {
            Self::IpcChannel => Rights::IPC_BASIC,
            Self::IpcEndpoint => Rights::EXEC,
            Self::ShmRegion => Rights::READ_WRITE,
            Self::File => Rights::READ,
            Self::Directory => Rights::READ,
            Self::Process => Rights::WRITE,
            Self::CryptoKey => Rights::EXEC,
        }
    }

    /// Human-readable name for audit logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::IpcChannel => "IpcChannel",
            Self::IpcEndpoint => "IpcEndpoint",
            Self::ShmRegion => "ShmRegion",
            Self::File => "File",
            Self::Directory => "Directory",
            Self::Process => "Process",
            Self::CryptoKey => "CryptoKey",
        }
    }
}

impl core::fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}
