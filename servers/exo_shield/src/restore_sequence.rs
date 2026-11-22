//! # `servers/exo_shield/src/restore_sequence.rs`
//!
//! ExoPhoenix Strate 1 parallel wave restoration (`R0 -> R1 -> R2`) and `t5` SLA capture
//! (Spec §3.2 & §5.3).
//!
//! - **Wave R0 (150 ms)**: `crypto_server`, `memory_server`, `ipc_broker` in parallel.
//! - **Wave R1 (250 ms)**: `vfs_server`, `device_server`, `fb_server` in parallel.
//! - **Wave R2 (150 ms)**: `init_server` ready -> capture `t5_strate1_ready` (END OF 1s SLA)
//!   -> asynchronously trigger Strate 2 relaunch in `init_server`.
//! - Total wave budget = 150 + 250 + 150 = 550 ms <= 750 ms hard limit.

/// Parallel restore budget for Wave R0 (`crypto_server`, `memory_server`, `ipc_broker`).
pub const WAVE_R0_BUDGET_MS: u64 = 150;
/// Parallel restore budget for Wave R1 (`vfs_server`, `device_server`, `fb_server`).
pub const WAVE_R1_BUDGET_MS: u64 = 250;
/// Restore budget for Wave R2 (`init_server`).
pub const WAVE_R2_BUDGET_MS: u64 = 150;

pub const WAVE_R0_SERVERS: &[&str] = &["crypto_server", "memory_server", "ipc_broker"];
pub const WAVE_R1_SERVERS: &[&str] = &["vfs_server", "device_server", "fb_server"];
pub const WAVE_R2_SERVERS: &[&str] = &["init_server"];

const _: () = assert!(
    WAVE_R0_BUDGET_MS + WAVE_R1_BUDGET_MS + WAVE_R2_BUDGET_MS <= 750,
    "Sum of Strate 1 restore waves R0 + R1 + R2 must stay <= 750 ms"
);

/// Result of executing the 3 Strate 1 parallel restore waves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreWaveOutcome {
    pub r0_ready: bool,
    pub r1_ready: bool,
    pub r2_ready: bool,
    /// Serialized TSC timestamp `t5` captured right after Wave R2 completes (end of 1s SLA).
    pub t5_strate1_ready_tsc: u64,
    /// Wall elapsed time across R0+R1+R2 in milliseconds.
    pub elapsed_ms: u64,
    /// Whether `init_server` was notified asynchronously to relaunch Strate 2 in background.
    pub strate2_async_triggered: bool,
}

impl RestoreWaveOutcome {
    #[inline]
    pub const fn all_waves_ready(&self) -> bool {
        self.r0_ready && self.r1_ready && self.r2_ready
    }
}

fn wait_wave_parallel<P, T>(
    servers: &'static [&'static str],
    budget_ms: u64,
    poll_ready: &mut P,
    now_ms: &mut T,
) -> bool
where
    P: FnMut(&'static str) -> bool,
    T: FnMut() -> u64,
{
    let start = now_ms();
    let deadline = start.saturating_add(budget_ms);
    let mut ready_mask = 0u8;
    let target_mask = ((1u16 << servers.len()) - 1) as u8;
    loop {
        for (idx, &svc) in servers.iter().enumerate() {
            let bit = 1u8 << idx;
            if (ready_mask & bit) == 0 && poll_ready(svc) {
                ready_mask |= bit;
            }
        }
        if ready_mask == target_mask {
            return true;
        }
        if now_ms() >= deadline {
            return false;
        }
    }
}

/// Executes Strate 1 parallel restoration waves `R0 -> R1 -> R2`, captures `t5` at the end
/// of Wave R2 (closing the <= 1 s SLA window), and triggers asynchronous Strate 2 relaunch.
pub fn execute_strate1_restore_waves<P, T, R, N>(
    mut poll_ready: P,
    mut now_ms: T,
    mut read_tsc: R,
    mut notify_init_strate2: N,
) -> RestoreWaveOutcome
where
    P: FnMut(&'static str) -> bool,
    T: FnMut() -> u64,
    R: FnMut() -> u64,
    N: FnMut() -> bool,
{
    let start_ms = now_ms();

    let r0_ready = wait_wave_parallel(WAVE_R0_SERVERS, WAVE_R0_BUDGET_MS, &mut poll_ready, &mut now_ms);
    let r1_ready = wait_wave_parallel(WAVE_R1_SERVERS, WAVE_R1_BUDGET_MS, &mut poll_ready, &mut now_ms);
    let r2_ready = wait_wave_parallel(WAVE_R2_SERVERS, WAVE_R2_BUDGET_MS, &mut poll_ready, &mut now_ms);

    // Capture `t5` right at the end of Wave R2 (Strate 1 ready) BEFORE Strate 2 async launch.
    let t5_strate1_ready_tsc = read_tsc();
    let elapsed_ms = now_ms().saturating_sub(start_ms);

    // Trigger Strate 2 asynchronous reconvergence in `init_server` (hors SLA).
    let strate2_async_triggered = notify_init_strate2();

    RestoreWaveOutcome {
        r0_ready,
        r1_ready,
        r2_ready,
        t5_strate1_ready_tsc,
        elapsed_ms,
        strate2_async_triggered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn waves_capture_t5_before_async_strate2_trigger() {
        let mut clock = 10u64;
        let tsc = AtomicU64::new(1000);
        let mut t5_seen_at_notify = 0u64;
        let out = execute_strate1_restore_waves(
            |_| true,
            || {
                clock += 5;
                clock
            },
            || tsc.fetch_add(500, Ordering::SeqCst) + 500,
            || {
                t5_seen_at_notify = tsc.load(Ordering::SeqCst);
                true
            },
        );
        assert!(out.all_waves_ready());
        assert!(out.strate2_async_triggered);
        assert_eq!(out.t5_strate1_ready_tsc, 1500);
        assert_eq!(t5_seen_at_notify, out.t5_strate1_ready_tsc);
        assert!(WAVE_R0_BUDGET_MS + WAVE_R1_BUDGET_MS + WAVE_R2_BUDGET_MS <= 750);
    }
}
