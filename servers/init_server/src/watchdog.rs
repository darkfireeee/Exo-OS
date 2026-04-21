use super::{syscall, Service};

const MAX_MISSED_POLLS: u8 = 3;

pub struct InitWatchdog<const N: usize> {
    missed_polls: [u8; N],
}

impl<const N: usize> InitWatchdog<N> {
    pub const fn new() -> Self {
        Self {
            missed_polls: [0; N],
        }
    }

    pub fn observe_spawn(&mut self, idx: usize) {
        if idx < N {
            self.missed_polls[idx] = 0;
        }
    }

    pub fn observe_stop(&mut self, idx: usize) {
        if idx < N {
            self.missed_polls[idx] = 0;
        }
    }

    pub fn check(&mut self, idx: usize, service: &Service) -> bool {
        if idx >= N {
            return true;
        }

        let pid = service.current_pid();
        if pid == 0 {
            self.missed_polls[idx] = 0;
            return true;
        }

        let alive = unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, 0) == 0 };
        if alive {
            self.missed_polls[idx] = 0;
            true
        } else {
            self.missed_polls[idx] = self.missed_polls[idx].saturating_add(1);
            self.missed_polls[idx] < MAX_MISSED_POLLS
        }
    }
}
