//! Stubs temporaires pour signaux POSIX (Phase 0)
//! En Phase 1, ces types seront remplacés par crate::posix_x::signals

/// Ensemble de signaux (stub Phase 0)
#[derive(Debug, Clone, Copy)]
pub struct SigSet(u64);

impl SigSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn contains(&self, sig: u32) -> bool {
        if sig >= 64 {
            return false;
        }
        (self.0 & (1 << sig)) != 0
    }

    pub fn insert(&mut self, sig: u32) {
        if sig < 64 {
            self.0 |= 1 << sig;
        }
    }

    pub fn add(&mut self, sig: u32) {
        self.insert(sig);
    }

    pub fn remove(&mut self, sig: u32) {
        if sig < 64 {
            self.0 &= !(1 << sig);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn first(&self) -> Option<u32> {
        if self.0 == 0 {
            None
        } else {
            Some(self.0.trailing_zeros())
        }
    }
}

/// Action d'un signal (stub Phase 0)
#[derive(Debug, Clone, Copy)]
pub enum SigAction {
    Default,
    Ignore,
    Handler {
        handler: usize,
        mask: SigSet,
    },
}

/// Stack frame pour signal (stub Phase 0)
/// Doit correspondre au format attendu par thread.rs
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalStackFrame {
    pub context: crate::scheduler::thread::ThreadContext,
    pub sig: u32,
    pub ret_addr: u64,
}
