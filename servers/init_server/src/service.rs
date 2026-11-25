//! Pure Ring 1 supervised service state machine with exponential restart backoff.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const SERVICE_STABLE_MS: u64 = 5_000;

#[cfg(target_os = "none")]
fn default_now_ms() -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    const CLOCK_MONOTONIC: u64 = 1;
    let mut ts = Timespec::default();
    let rc = unsafe {
        exo_syscall_abi::syscall2(
            exo_syscall_abi::SYS_CLOCK_GETTIME,
            CLOCK_MONOTONIC,
            &mut ts as *mut Timespec as u64,
        )
    };
    if rc != 0 || ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000)
}

#[cfg(not(target_os = "none"))]
fn default_now_ms() -> u64 {
    0
}

/// Descripteur d'un service supervisé par `init_server` (PID 1).
pub struct Service {
    #[allow(dead_code)]
    pub name: &'static str,
    pub bin_path: &'static [u8],
    pub pid: AtomicU32,
    pub restart_delay_ticks: AtomicU32,
    pub spawn_time_ms: AtomicU64,
    pub disabled: AtomicBool,
    pub dead: AtomicBool,
    pub ready: AtomicBool,
}

impl Service {
    pub const fn new(name: &'static str, bin: &'static [u8]) -> Self {
        Self {
            name,
            bin_path: bin,
            pid: AtomicU32::new(0),
            restart_delay_ticks: AtomicU32::new(1),
            spawn_time_ms: AtomicU64::new(0),
            disabled: AtomicBool::new(false),
            dead: AtomicBool::new(false),
            ready: AtomicBool::new(false),
        }
    }

    pub fn current_pid(&self) -> u32 {
        self.pid.load(Ordering::Acquire)
    }

    pub fn delay(&self) -> u32 {
        self.restart_delay_ticks.load(Ordering::Relaxed)
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Acquire)
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn set_pid_at(&self, pid: u32, now_ms: u64) {
        self.pid.store(pid, Ordering::Release);
        self.disabled.store(false, Ordering::Release);
        self.dead.store(false, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        self.spawn_time_ms.store(now_ms, Ordering::Release);
    }

    pub fn set_pid(&self, pid: u32) {
        self.set_pid_at(pid, default_now_ms());
    }

    pub fn mark_ready(&self) {
        if self.current_pid() != 0 {
            self.ready.store(true, Ordering::Release);
        }
    }

    pub fn enable(&self) {
        self.disabled.store(false, Ordering::Release);
        self.dead.store(false, Ordering::Release);
    }

    pub fn disable(&self) {
        self.disabled.store(true, Ordering::Release);
        self.dead.store(true, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        self.pid.store(0, Ordering::Release);
        self.spawn_time_ms.store(0, Ordering::Release);
        self.restart_delay_ticks.store(1, Ordering::Relaxed);
    }

    pub fn mark_dead_at(&self, now_ms: u64) {
        let spawn = self.spawn_time_ms.load(Ordering::Acquire);
        self.pid.store(0, Ordering::Release);
        self.dead.store(true, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        self.spawn_time_ms.store(0, Ordering::Release);
        if spawn != 0 && now_ms.saturating_sub(spawn) >= SERVICE_STABLE_MS {
            self.restart_delay_ticks.store(1, Ordering::Relaxed);
        } else {
            let d = self.restart_delay_ticks.load(Ordering::Relaxed).max(1);
            self.restart_delay_ticks
                .store(d.saturating_mul(2).min(32), Ordering::Relaxed);
        }
    }

    pub fn mark_dead(&self) {
        self.mark_dead_at(default_now_ms());
    }

    pub fn note_alive_at(&self, now_ms: u64) {
        let pid = self.current_pid();
        if pid == 0 {
            return;
        }
        let spawn = self.spawn_time_ms.load(Ordering::Acquire);
        if spawn != 0 && now_ms.saturating_sub(spawn) >= SERVICE_STABLE_MS {
            self.restart_delay_ticks.store(1, Ordering::Relaxed);
        }
    }

    pub fn note_alive(&self) {
        self.note_alive_at(default_now_ms());
    }
}
