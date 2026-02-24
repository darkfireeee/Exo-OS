//! protocols/ — Protocoles UEFI utilisés par exo-boot.
//!
//! Chaque sous-module encapsule un protocole UEFI avec des wrappers
//! ergonomiques, des logs de diagnostic et des vérifications d'erreur.
//!
//! Protocoles implémentés :
//!   - `graphics`      : GOP — Graphics Output Protocol (framebuffer)
//!   - `file`          : EFI_FILE_PROTOCOL — lecture FAT32/ESP
//!   - `loaded_image`  : EFI_LOADED_IMAGE — infos sur le bootloader lui-même
//!   - `rng`           : EFI_RNG_PROTOCOL — entropy initiale (KASLR + CSPRNG)

pub mod file;
pub mod graphics;
pub mod loaded_image;
pub mod rng;
