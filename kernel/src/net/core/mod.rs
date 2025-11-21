//! Network core layer

use super::{NetResult, NetError};

/// Network sockets
pub mod socket;

/// Network devices
pub mod device;

/// Network buffers
pub mod buffer;

/// Initialize network core
pub fn init() -> NetResult<()> {
    log::debug!("Network core initialized");
    Ok(())
}
