//! Canonical architecture constants audited before the v0.2.0 boot path.

/// Maximum number of cores represented by ExoPhoenix SSR core masks.
pub const MAX_CORES_LAYOUT: usize = 256;

/// Alias used by audit tooling for SSR-specific layout checks.
pub const SSR_MAX_CORES_LAYOUT: usize = MAX_CORES_LAYOUT;

/// Number of u64 words needed to represent one bit per supported core.
pub const CORE_MASK_WORDS: usize = (MAX_CORES_LAYOUT + 63) / 64;

/// Maximum inline IPC message payload stored in one kernel ring slot.
pub const MAX_MSG_SIZE: usize = 240;

/// Maximum payload reserved for APIs that need extra protocol headers.
pub const IPC_INLINE_MAX: usize = 200;

/// Maximum process table size used by the current process registry.
pub const MAX_PROCESSES: usize = 32_768;

/// Minimum accepted fixed ELF Ring3 load address.
pub const USER_ELF_BASE_MIN: u64 = 0x0040_0000;

/// Minimum direct-physmap coverage required by CORR-76.
pub const PHYSMAP_INITIAL_COVERAGE_BYTES: u64 = 1 << 30;

const _: () = assert!(
    CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT,
    "CORE_MASK_WORDS incoherent with MAX_CORES_LAYOUT"
);

const _: () = assert!(
    SSR_MAX_CORES_LAYOUT == MAX_CORES_LAYOUT,
    "SSR_MAX_CORES_LAYOUT must track MAX_CORES_LAYOUT"
);

const _: () = assert!(
    IPC_INLINE_MAX < MAX_MSG_SIZE,
    "IPC_INLINE_MAX must leave room inside MAX_MSG_SIZE"
);

const _: () = assert!(
    USER_ELF_BASE_MIN <= 0x0040_0000,
    "USER_ELF_BASE_MIN too high for standard x86_64 ELF"
);
