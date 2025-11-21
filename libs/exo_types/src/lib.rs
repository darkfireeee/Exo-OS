#![no_std]

extern crate alloc;

pub mod address;
pub mod capability;
pub mod error;

// Réexportations
pub use address::{PhysAddr, VirtAddr};
pub use capability::{Capability, CapabilityMetadata, CapabilityType, Rights};
pub use error::{ExoError, Result};

// Initialisation globale
pub fn init() {
    log::trace!("exo_types initialized");
}
