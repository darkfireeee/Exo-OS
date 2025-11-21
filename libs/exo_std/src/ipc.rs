// libs/exo_std/src/ipc.rs
pub use exo_ipc::*;

/// Envoie un message
pub fn send(dest: u32, data: &[u8]) -> crate::Result<()> {
    Ok(()) // TODO
}
