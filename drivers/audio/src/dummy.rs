//! Pilote audio muet (Dummy / Sound-Optional Fallback)
//!
//! Conforme à `SPEC-AUDIO-STRATA.md` §6 :
//! "Si audio_server est absent ou en erreur, aucun composant ne panique.
//!  Le système est sound-optional : il fonctionne silencieusement si aucun
//!  hardware audio n'est détecté."

use exo_os_drivers::{AudioDevice, AudioError};

pub struct DummyAudioDevice;

impl DummyAudioDevice {
    pub const fn new() -> Self {
        Self
    }
}

impl AudioDevice for DummyAudioDevice {
    fn play_pcm(&self, _data: &[i16], _rate: u32, _channels: u8) -> Result<(), AudioError> {
        // Fallback silencieux sans allocation ni erreur
        Ok(())
    }

    fn stop(&self) {}
}
