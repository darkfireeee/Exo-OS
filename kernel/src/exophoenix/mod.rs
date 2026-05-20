//! # ExoPhoenix — noyau sentinelle (Kernel B)
//!
//! Ce module centralise l'état partagé et les primitives communes entre
//! les composants ExoPhoenix. Les implémentations détaillées Stage 0 / sentinel
//! seront branchées incrémentalement.

use core::sync::atomic::{AtomicU8, Ordering};

pub mod forge;
pub mod handoff;
pub mod interrupts;
pub mod isolate;
pub mod resurrection;
pub mod sentinel;
pub mod ssr;
pub mod stage0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PhoenixState {
    BootStage0 = 0,
    Normal = 1,
    Threat = 2,
    IsolationSoft = 3,
    IsolationHard = 4,
    Certif = 5,
    Restore = 6,
    Degraded = 7,
    Emergency = 8,
    NetworkDraining = 9,
    NetworkSerialized = 10,
}

pub static PHOENIX_STATE: AtomicU8 = AtomicU8::new(PhoenixState::BootStage0 as u8);

impl PhoenixState {
    #[inline(always)]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::BootStage0),
            1 => Some(Self::Normal),
            2 => Some(Self::Threat),
            3 => Some(Self::IsolationSoft),
            4 => Some(Self::IsolationHard),
            5 => Some(Self::Certif),
            6 => Some(Self::Restore),
            7 => Some(Self::Degraded),
            8 => Some(Self::Emergency),
            9 => Some(Self::NetworkDraining),
            10 => Some(Self::NetworkSerialized),
            _ => None,
        }
    }
}

#[inline(always)]
pub fn set_state(state: PhoenixState) {
    PHOENIX_STATE.store(state as u8, Ordering::Release);
}

#[inline(always)]
pub fn try_set_state_raw(raw: u8) -> bool {
    if let Some(state) = PhoenixState::from_raw(raw) {
        set_state(state);
        true
    } else {
        false
    }
}

#[inline(always)]
pub fn state() -> PhoenixState {
    PhoenixState::from_raw(PHOENIX_STATE.load(Ordering::Acquire)).unwrap_or(PhoenixState::Emergency)
}

#[inline(always)]
pub(crate) fn take_slot_once(seen: &mut [u64; 4], slot: usize) -> bool {
    if slot >= ssr::MAX_CORES {
        return false;
    }

    let word = slot / u64::BITS as usize;
    let bit = 1u64 << (slot % u64::BITS as usize);
    let was_seen = seen[word] & bit != 0;
    seen[word] |= bit;
    !was_seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phoenix_state_wire_values_roundtrip() {
        for raw in 0u8..=10 {
            let state = PhoenixState::from_raw(raw).expect("known Phoenix state");
            assert_eq!(state as u8, raw);
        }
        assert!(PhoenixState::from_raw(11).is_none());
        assert!(PhoenixState::from_raw(u8::MAX).is_none());
    }

    #[test]
    fn try_set_state_raw_rejects_unknown_values_without_clobbering_state() {
        set_state(PhoenixState::Normal);

        assert!(!try_set_state_raw(250));
        assert_eq!(state(), PhoenixState::Normal);

        assert!(try_set_state_raw(PhoenixState::NetworkDraining as u8));
        assert_eq!(state(), PhoenixState::NetworkDraining);
    }

    #[test]
    fn take_slot_once_rejects_out_of_range_and_duplicates() {
        let mut seen = [0u64; 4];

        assert!(take_slot_once(&mut seen, 0));
        assert!(!take_slot_once(&mut seen, 0));
        assert!(take_slot_once(&mut seen, 127));
        assert!(!take_slot_once(&mut seen, ssr::MAX_CORES));
    }
}
