//! # ExoPhoenix — noyau sentinelle (Kernel B)
//!
//! Ce module centralise l'état partagé et les primitives communes entre
//! les composants ExoPhoenix. Les implémentations détaillées Stage 0 / sentinel
//! seront branchées incrémentalement.

use core::sync::atomic::AtomicU8;

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
}

pub static PHOENIX_STATE: AtomicU8 = AtomicU8::new(PhoenixState::BootStage0 as u8);

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
