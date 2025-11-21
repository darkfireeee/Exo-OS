// libs/exo_std/src/thread.rs
use core::time::Duration;

/// ID de thread
pub type Tid = u32;

/// Endort le thread actuel
pub fn sleep(duration: Duration) {
    crate::time::sleep(duration);
}

/// ID du thread actuel
pub fn id() -> Tid {
    0 // TODO
}

/// Cède la main au scheduler
pub fn yield_now() {
    // TODO
}
