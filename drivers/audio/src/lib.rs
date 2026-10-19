//! # exo_audio_drivers — Pilotes audio matériel Ring 3 pour Exo-OS
//!
//! Conforme à `SPEC-AUDIO-STRATA.md` :
//! - Intel HDA (High Definition Audio, PCI 0x04:0x03)
//! - VirtIO Sound (VirtIO Device ID 25)
//! - Dummy audio (Sound-optional silent fallback)
//! - Synthèse PCM purement entière (#![no_std], SRV-02)

#![no_std]

pub mod dummy;
pub mod hda;
pub mod synth;
pub mod virtio_sound;

pub use dummy::DummyAudioDevice;
pub use hda::HdaDriver;
pub use virtio_sound::VirtioSoundDriver;

pub use exo_os_drivers::{AudioDevice, AudioError, DeviceHandle, DeviceInfo, DriverError, ExoDriver};
