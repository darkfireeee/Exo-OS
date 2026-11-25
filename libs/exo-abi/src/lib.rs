#![no_std]
//! `exo-abi` — Leaf `#![no_std]` contract crate for Exo-OS v0.2.0 ("Strata").
//!
//! Zero internal dependencies: safe to import from Ring 0 (`kernel`), Ring 1
//! (`drivers`, `servers`), Ring 3 (`libs`, `loader`, `userspace`), and host tests.

pub mod cap;
pub mod errno;
pub mod numbers;
pub mod posix_consts;
pub mod syscall_arch;

pub use cap::*;
pub use errno::*;
pub use numbers::*;
pub use posix_consts::*;
pub use syscall_arch::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phoenix_force_aliases_are_identical() {
        assert_eq!(SYS_PHOENIX_FORCE, 523);
        assert_eq!(SYS_EXO_PHOENIX_FORCE, SYS_PHOENIX_FORCE);
        assert_eq!(SYS_EXO_RESURRECT, SYS_PHOENIX_FORCE);
    }

    #[test]
    fn negative_and_positive_errno_match() {
        assert_eq!(EINVAL, -(errno::positive::EINVAL as i64));
        assert_eq!(ENOSYS, -(errno::positive::ENOSYS as i64));
        assert_eq!(ETIMEDOUT, -(errno::positive::ETIMEDOUT as i64));
    }

    #[test]
    fn cap_object_discriminants_match_ring0() {
        assert_eq!(CapObjectType::IpcEndpoint as u32, 1);
        assert_eq!(CapObjectType::MemoryRegion as u32, 2);
    }
}
