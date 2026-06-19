//! disk/ — Accès disque et table de partitions du bootloader.
//!
//! - `gpt` : parsing GPT/MBR réel via la crate partagée `exo-partition`
//!   (même code que le kernel) + adaptateur `EFI_BLOCK_IO_PROTOCOL`.

pub mod gpt;
