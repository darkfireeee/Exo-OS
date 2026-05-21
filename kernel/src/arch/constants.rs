//! Canonical architecture constants audited before the v0.2.0 boot path.

/// Initial direct-physmap coverage installed by the boot page tables.
pub const PHYSMAP_INITIAL_COVERAGE: usize = 1 << 30;

/// Same coverage as a byte count for physical-address arithmetic.
pub const PHYSMAP_INITIAL_COVERAGE_BYTES: u64 = PHYSMAP_INITIAL_COVERAGE as u64;

/// Physical base of the ExoPhoenix Shared State Region.
pub const SSR_PHYS_BASE: u64 = 0x0100_0000;

/// Size of the reserved SSR physical window.
pub const SSR_PHYS_SIZE: u64 = 0x1_0000;

/// Exclusive end of the reserved SSR physical window.
pub const SSR_PHYS_END: u64 = SSR_PHYS_BASE + SSR_PHYS_SIZE;

/// Maximum number of cores represented by ExoPhoenix SSR core masks.
pub const MAX_CORES_LAYOUT: usize = 256;

/// Maximum number of CPUs expected in the current runtime boot profile.
pub const MAX_CORES_RUNTIME: usize = 64;

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

/// Maximum endpoint table size shared by IPC-facing contracts.
pub const MAX_ENDPOINTS: usize = 8_192;

/// Minimum accepted fixed ELF Ring3 load address.
pub const USER_ELF_BASE_MIN: u64 = 0x0040_0000;

/// Current top of the initial Ring3 stack window.
pub const USER_STACK_TOP: u64 = crate::memory::core::layout::USER_STACK_TOP.as_u64();

/// First canonical high-half kernel virtual address.
pub const KERNEL_BASE: u64 = crate::memory::core::layout::PHYS_MAP_BASE.as_u64();

/// ExoKairos budget accounting window, in nanoseconds.
pub const KAIROS_WINDOW_NS: u64 = 1_000_000_000;

/// ExoKairos throttles once the current window reaches 100% of budget.
pub const KAIROS_THROTTLE_PCT: u64 = 100;

/// ExoKairos kills once two full windows of budget have been consumed.
pub const KAIROS_KILL_PCT: u64 = 200;

const _: () = assert!(
    PHYSMAP_INITIAL_COVERAGE == 0x4000_0000,
    "PHYSMAP_INITIAL_COVERAGE must stay at 1 GiB"
);

const _: () = assert!(
    SSR_PHYS_END - SSR_PHYS_BASE == SSR_PHYS_SIZE,
    "SSR physical window is incoherent"
);

const _: () = assert!(
    CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT,
    "CORE_MASK_WORDS incoherent with MAX_CORES_LAYOUT"
);

const _: () = assert!(
    MAX_CORES_RUNTIME <= MAX_CORES_LAYOUT,
    "MAX_CORES_RUNTIME exceeds MAX_CORES_LAYOUT"
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
    USER_ELF_BASE_MIN < USER_STACK_TOP,
    "USER_ELF_BASE_MIN must remain below USER_STACK_TOP"
);

const _: () = assert!(
    KERNEL_BASE >= 0xFFFF_8000_0000_0000,
    "KERNEL_BASE must remain in the canonical high half"
);

const _: () = assert!(
    MAX_PROCESSES <= 65_536,
    "MAX_PROCESSES is too large for current SSR/process table assumptions"
);

const _: () = assert!(KAIROS_WINDOW_NS > 0, "KAIROS_WINDOW_NS must be non-zero");

const _: () = assert!(
    KAIROS_THROTTLE_PCT == 100,
    "KAIROS_THROTTLE_PCT must represent one full window"
);

const _: () = assert!(
    KAIROS_KILL_PCT == 200,
    "KAIROS_KILL_PCT must represent two full windows"
);
