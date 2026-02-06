//! Capability-based security primitives
//!
//! Zero-allocation, high-performance capability system for Exo-OS.
//! Implements object capabilities with fine-grained permissions.

#![allow(missing_docs)]

use bitflags::bitflags;
use core::fmt;

bitflags! {
    /// Fine-grained access rights for capabilities
    ///
    /// Each bit represents a specific permission. Rights can be combined
    /// using bitwise operations (|, &, ^).
    pub struct Rights: u32 {
        const NONE        = 0b0000_0000;
        const READ        = 0b0000_0001;
        const WRITE       = 0b0000_0010;
        const EXECUTE     = 0b0000_0100;
        const DELETE      = 0b0000_1000;
        const METADATA    = 0b0001_0000;
        const CREATE_FILE = 0b0010_0000;
        const CREATE_DIR  = 0b0100_0000;
        const NET_BIND    = 0b1000_0000;
        const NET_CONNECT = 0b0001_0000_0000;
        const IPC_SEND    = 0b0010_0000_0000;
        const IPC_RECV    = 0b0100_0000_0000;
        const ADMIN       = 0b1000_0000_0000;

        // Permissions IPC additionnelles (compatibilité exo_ipc)
        const IPC_CREATE  = 0b0001_0000_0000_0000;  // Créer des canaux IPC
        const IPC_DESTROY = 0b0010_0000_0000_0000;  // Détruire des canaux IPC
        const IPC_DELEGATE= 0b0100_0000_0000_0000;  // Déléguer des capabilities

        /// Standard file permissions (read, write, metadata)
        const FILE_STANDARD = Self::READ.bits | Self::WRITE.bits | Self::METADATA.bits;

        /// Standard directory permissions (file + create)
        const DIR_STANDARD = Self::FILE_STANDARD.bits | Self::CREATE_FILE.bits | Self::CREATE_DIR.bits;

        /// Standard socket permissions
        const SOCKET_STANDARD = Self::READ.bits | Self::WRITE.bits | Self::NET_BIND.bits | Self::NET_CONNECT.bits;

        /// IPC permissions (send + receive)
        const IPC_STANDARD = Self::IPC_SEND.bits | Self::IPC_RECV.bits;

        /// IPC permissions complètes (send, receive, create, destroy, delegate)
        const IPC_FULL = Self::IPC_SEND.bits | Self::IPC_RECV.bits | Self::IPC_CREATE.bits | Self::IPC_DESTROY.bits | Self::IPC_DELEGATE.bits;

        /// All permissions (for privileged operations)
        const ALL = u32::MAX;
    }
}

impl Rights {
    /// Check if rights include read permission
    #[inline(always)]
    pub const fn can_read(self) -> bool {
        self.bits() & Self::READ.bits != 0
    }

    /// Check if rights include write permission
    #[inline(always)]
    pub const fn can_write(self) -> bool {
        self.bits() & Self::WRITE.bits != 0
    }

    /// Check if rights include execute permission
    #[inline(always)]
    pub const fn can_execute(self) -> bool {
        self.bits() & Self::EXECUTE.bits != 0
    }

    /// Check if rights include admin permission
    #[inline(always)]
    pub const fn is_admin(self) -> bool {
        self.bits() & Self::ADMIN.bits != 0
    }

    /// Reduce rights (attenuation)
    #[inline(always)]
    pub const fn attenuate(self, mask: Rights) -> Self {
        Self::from_bits_truncate(self.bits() & mask.bits())
    }

    /// Check if has all required rights
    #[inline(always)]
    pub const fn has_all(self, required: Rights) -> bool {
        (self.bits() & required.bits()) == required.bits()
    }

    /// Check if has any of the required rights
    #[inline(always)]
    pub const fn has_any(self, required: Rights) -> bool {
        (self.bits() & required.bits()) != 0
    }
}

/// Capability object type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CapabilityType {
    /// File capability
    File = 0,
    /// Directory capability
    Directory = 1,
    /// Memory region capability
    Memory = 2,
    /// Process capability
    Process = 3,
    /// Thread capability
    Thread = 4,
    /// IPC channel capability
    IpcChannel = 5,
    /// Network socket capability
    NetworkSocket = 6,
    /// Device capability
    Device = 7,
    /// Cryptographic key capability
    Key = 8,
}

impl CapabilityType {
    /// Get default rights for this capability type
    #[inline]
    pub const fn default_rights(self) -> Rights {
        match self {
            Self::File => Rights::FILE_STANDARD,
            Self::Directory => Rights::DIR_STANDARD,
            Self::NetworkSocket => Rights::SOCKET_STANDARD,
            Self::IpcChannel => Rights::IPC_STANDARD,
            Self::Memory => Rights::from_bits_truncate(
                Rights::READ.bits | Rights::WRITE.bits | Rights::EXECUTE.bits
            ),
            Self::Process | Self::Thread => Rights::from_bits_truncate(
                Rights::READ.bits | Rights::METADATA.bits
            ),
            Self::Device => Rights::from_bits_truncate(
                Rights::READ.bits | Rights::WRITE.bits
            ),
            Self::Key => Rights::from_bits_truncate(
                Rights::READ.bits | Rights::METADATA.bits
            ),
        }
    }

    /// Convert to raw byte value
    #[inline(always)]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Convert from raw byte value
    #[inline]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::File),
            1 => Some(Self::Directory),
            2 => Some(Self::Memory),
            3 => Some(Self::Process),
            4 => Some(Self::Thread),
            5 => Some(Self::IpcChannel),
            6 => Some(Self::NetworkSocket),
            7 => Some(Self::Device),
            8 => Some(Self::Key),
            _ => None,
        }
    }

    /// Get type name as string
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Directory => "Directory",
            Self::Memory => "Memory",
            Self::Process => "Process",
            Self::Thread => "Thread",
            Self::IpcChannel => "IpcChannel",
            Self::NetworkSocket => "NetworkSocket",
            Self::Device => "Device",
            Self::Key => "Key",
        }
    }
}

impl fmt::Display for CapabilityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compact metadata flags (16 bits)
///
/// Packed representation to avoid allocations:
/// - Bits 0-7: Type-specific flags
/// - Bits 8-15: Reserved for future use
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MetadataFlags(u16);

impl MetadataFlags {
    /// No flags set
    pub const NONE: Self = Self(0);

    /// Memory is executable
    pub const MEM_EXECUTABLE: Self = Self(1 << 0);

    /// Memory is shared
    pub const MEM_SHARED: Self = Self(1 << 1);

    /// File is compressed
    pub const FILE_COMPRESSED: Self = Self(1 << 2);

    /// File is encrypted
    pub const FILE_ENCRYPTED: Self = Self(1 << 3);

    /// Create new flags
    #[inline(always)]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Get raw value
    #[inline(always)]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Check if flag is set
    #[inline(always)]
    pub const fn has(self, flag: Self) -> bool {
        (self.0 & flag.0) != 0
    }

    /// Set flag
    #[inline(always)]
    pub const fn set(self, flag: Self) -> Self {
        Self(self.0 | flag.0)
    }

    /// Clear flag
    #[inline(always)]
    pub const fn clear(self, flag: Self) -> Self {
        Self(self.0 & !flag.0)
    }
}

/// Capability metadata (zero-allocation)
///
/// Uses compact representation instead of String allocations.
/// For paths, store hash or index into static table instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CapabilityMetadata {
    /// Size for memory/file objects (0 = unused)
    size: u64,

    /// Path hash or identifier (0 = no path)
    path_hash: u64,

    /// Type-specific flags
    flags: MetadataFlags,
}

impl CapabilityMetadata {
    /// Empty metadata
    pub const EMPTY: Self = Self {
        size: 0,
        path_hash: 0,
        flags: MetadataFlags::NONE,
    };

    /// Create metadata with size
    #[inline(always)]
    pub const fn with_size(size: u64) -> Self {
        Self {
            size,
            path_hash: 0,
            flags: MetadataFlags::NONE,
        }
    }

    /// Create metadata with path hash
    #[inline(always)]
    pub const fn with_path_hash(hash: u64) -> Self {
        Self {
            size: 0,
            path_hash: hash,
            flags: MetadataFlags::NONE,
        }
    }

    /// Create metadata with flags
    #[inline(always)]
    pub const fn with_flags(flags: MetadataFlags) -> Self {
        Self {
            size: 0,
            path_hash: 0,
            flags,
        }
    }

    /// Get size
    #[inline(always)]
    pub const fn size(self) -> u64 {
        self.size
    }

    /// Get path hash
    #[inline(always)]
    pub const fn path_hash(self) -> u64 {
        self.path_hash
    }

    /// Get flags
    #[inline(always)]
    pub const fn flags(self) -> MetadataFlags {
        self.flags
    }

    /// Set size
    #[inline(always)]
    pub const fn set_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// Set path hash
    #[inline(always)]
    pub const fn set_path_hash(mut self, hash: u64) -> Self {
        self.path_hash = hash;
        self
    }

    /// Set flags
    #[inline(always)]
    pub const fn set_flags(mut self, flags: MetadataFlags) -> Self {
        self.flags = flags;
        self
    }

    /// Check if metadata is empty
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.size == 0 && self.path_hash == 0 && self.flags.0 == 0
    }
}

/// Capability object (Copy type, no allocations)
///
/// Compact representation:
/// - 8 bytes: capability ID
/// - 1 byte: type
/// - 4 bytes: rights
/// - 24 bytes: metadata
/// Total: 37 bytes (padded to 40 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    /// Unique capability identifier
    id: u64,

    /// Capability type
    cap_type: CapabilityType,

    /// Access rights
    rights: Rights,

    /// Optional metadata
    metadata: CapabilityMetadata,
}

impl Capability {
    /// Create new capability
    #[inline(always)]
    pub const fn new(id: u64, cap_type: CapabilityType, rights: Rights) -> Self {
        Self {
            id,
            cap_type,
            rights,
            metadata: CapabilityMetadata::EMPTY,
        }
    }

    /// Create capability with default rights for type
    #[inline]
    pub const fn with_defaults(id: u64, cap_type: CapabilityType) -> Self {
        Self::new(id, cap_type, cap_type.default_rights())
    }

    /// Create capability with metadata
    #[inline(always)]
    pub const fn with_metadata(
        id: u64,
        cap_type: CapabilityType,
        rights: Rights,
        metadata: CapabilityMetadata,
    ) -> Self {
        Self {
            id,
            cap_type,
            rights,
            metadata,
        }
    }

    /// Get capability ID
    #[inline(always)]
    pub const fn id(self) -> u64 {
        self.id
    }

    /// Get capability type
    #[inline(always)]
    pub const fn cap_type(self) -> CapabilityType {
        self.cap_type
    }

    /// Get rights
    #[inline(always)]
    pub const fn rights(self) -> Rights {
        self.rights
    }

    /// Get metadata
    #[inline(always)]
    pub const fn metadata(self) -> CapabilityMetadata {
        self.metadata
    }

    /// Check if has required rights
    #[inline(always)]
    pub const fn has_rights(self, required: Rights) -> bool {
        self.rights.has_all(required)
    }

    /// Check if has any of the rights
    #[inline(always)]
    pub const fn has_any_rights(self, required: Rights) -> bool {
        self.rights.has_any(required)
    }

    /// Attenuate capability (reduce rights)
    ///
    /// Returns new capability with reduced rights. Original is unchanged.
    /// This is the core of capability-based security: you can only
    /// reduce permissions, never increase them.
    #[inline(always)]
    pub const fn attenuate(self, new_rights: Rights) -> Self {
        Self {
            id: self.id,
            cap_type: self.cap_type,
            rights: self.rights.attenuate(new_rights),
            metadata: self.metadata,
        }
    }

    /// Set metadata
    #[inline(always)]
    pub const fn set_metadata(mut self, metadata: CapabilityMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Check if capability is valid (non-zero ID)
    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.id != 0
    }

    /// Null capability (invalid)
    pub const NULL: Self = Self {
        id: 0,
        cap_type: CapabilityType::File,
        rights: Rights::NONE,
        metadata: CapabilityMetadata::EMPTY,
    };

    /// System capability with all permissions
    pub const SYSTEM: Self = Self {
        id: 1,
        cap_type: CapabilityType::Process,
        rights: Rights::ALL,
        metadata: CapabilityMetadata::EMPTY,
    };

    /// Create a system capability with all permissions
    #[inline(always)]
    pub const fn system() -> Self {
        Self::SYSTEM
    }
}

impl Default for Capability {
    #[inline]
    fn default() -> Self {
        Self::NULL
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Cap(id={:#x}, type={}, rights={:?})",
            self.id, self.cap_type, self.rights
        )
    }
}

/// Simple hash function for paths (FNV-1a)
#[inline]
pub const fn hash_path(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    hash
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;

    #[test]
    fn test_rights_basic() {
        let r = Rights::READ | Rights::WRITE;
        assert!(r.can_read());
        assert!(r.can_write());
        assert!(!r.can_execute());
        assert!(!r.is_admin());
    }

    #[test]
    fn test_rights_attenuation() {
        let original = Rights::FILE_STANDARD;
        let reduced = original.attenuate(Rights::READ);
        
        assert!(original.can_read());
        assert!(original.can_write());
        
        assert!(reduced.can_read());
        assert!(!reduced.can_write());
    }

    #[test]
    fn test_rights_has_all() {
        let r = Rights::READ | Rights::WRITE;
        assert!(r.has_all(Rights::READ));
        assert!(r.has_all(Rights::WRITE));
        assert!(r.has_all(Rights::READ | Rights::WRITE));
        assert!(!r.has_all(Rights::READ | Rights::WRITE | Rights::EXECUTE));
    }

    #[test]
    fn test_rights_has_any() {
        let r = Rights::READ | Rights::WRITE;
        assert!(r.has_any(Rights::READ));
        assert!(r.has_any(Rights::EXECUTE | Rights::READ));
        assert!(!r.has_any(Rights::EXECUTE | Rights::DELETE));
    }

    #[test]
    fn test_capability_type_conversions() {
        assert_eq!(CapabilityType::File.as_u8(), 0);
        assert_eq!(CapabilityType::from_u8(0), Some(CapabilityType::File));
        assert_eq!(CapabilityType::from_u8(99), None);
    }

    #[test]
    fn test_capability_type_defaults() {
        assert_eq!(CapabilityType::File.default_rights(), Rights::FILE_STANDARD);
        assert_eq!(CapabilityType::Directory.default_rights(), Rights::DIR_STANDARD);
    }

    #[test]
    fn test_metadata_flags() {
        let flags = MetadataFlags::NONE
            .set(MetadataFlags::MEM_EXECUTABLE)
            .set(MetadataFlags::MEM_SHARED);
        
        assert!(flags.has(MetadataFlags::MEM_EXECUTABLE));
        assert!(flags.has(MetadataFlags::MEM_SHARED));
        assert!(!flags.has(MetadataFlags::FILE_COMPRESSED));
        
        let cleared = flags.clear(MetadataFlags::MEM_EXECUTABLE);
        assert!(!cleared.has(MetadataFlags::MEM_EXECUTABLE));
        assert!(cleared.has(MetadataFlags::MEM_SHARED));
    }

    #[test]
    fn test_capability_metadata() {
        let meta = CapabilityMetadata::with_size(4096)
            .set_path_hash(0x1234)
            .set_flags(MetadataFlags::MEM_EXECUTABLE);
        
        assert_eq!(meta.size(), 4096);
        assert_eq!(meta.path_hash(), 0x1234);
        assert!(meta.flags().has(MetadataFlags::MEM_EXECUTABLE));
        assert!(!meta.is_empty());
        
        assert!(CapabilityMetadata::EMPTY.is_empty());
    }

    #[test]
    fn test_capability_creation() {
        let cap = Capability::new(1, CapabilityType::File, Rights::READ);
        
        assert_eq!(cap.id(), 1);
        assert_eq!(cap.cap_type(), CapabilityType::File);
        assert_eq!(cap.rights(), Rights::READ);
        assert!(cap.is_valid());
        
        assert!(!Capability::NULL.is_valid());
    }

    #[test]
    fn test_capability_attenuation() {
        let cap = Capability::new(1, CapabilityType::File, Rights::FILE_STANDARD);
        
        assert!(cap.has_rights(Rights::READ));
        assert!(cap.has_rights(Rights::WRITE));
        
        let attenuated = cap.attenuate(Rights::READ);
        
        assert!(attenuated.has_rights(Rights::READ));
        assert!(!attenuated.has_rights(Rights::WRITE));
        assert_eq!(attenuated.id(), cap.id());
        assert_eq!(attenuated.cap_type(), cap.cap_type());
    }

    #[test]
    fn test_capability_with_defaults() {
        let cap = Capability::with_defaults(42, CapabilityType::Directory);
        
        assert_eq!(cap.id(), 42);
        assert_eq!(cap.cap_type(), CapabilityType::Directory);
        assert_eq!(cap.rights(), Rights::DIR_STANDARD);
    }

    #[test]
    fn test_capability_with_metadata() {
        let meta = CapabilityMetadata::with_size(8192);
        let cap = Capability::with_metadata(
            1,
            CapabilityType::Memory,
            Rights::READ | Rights::WRITE,
            meta,
        );
        
        assert_eq!(cap.metadata().size(), 8192);
    }

    #[test]
    fn test_hash_path() {
        let hash1 = hash_path(b"/home/user/file.txt");
        let hash2 = hash_path(b"/home/user/file.txt");
        let hash3 = hash_path(b"/home/user/other.txt");
        
        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_capability_is_copy() {
        let cap1 = Capability::new(1, CapabilityType::File, Rights::READ);
        let cap2 = cap1;
        
        assert_eq!(cap1.id(), cap2.id());
    }

    #[test]
    fn test_sizes() {
        assert_eq!(size_of::<Rights>(), 4);
        assert_eq!(size_of::<CapabilityType>(), 1);
        assert_eq!(size_of::<MetadataFlags>(), 2);
        assert_eq!(size_of::<CapabilityMetadata>(), 24);
        assert!(size_of::<Capability>() <= 40);
    }

    #[test]
    fn test_capability_default() {
        let cap = Capability::default();
        assert!(!cap.is_valid());
        assert_eq!(cap, Capability::NULL);
    }
}
