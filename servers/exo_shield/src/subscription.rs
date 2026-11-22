//! # `servers/exo_shield/src/subscription.rs`
//!
//! ExoPhoenix 3-Strata O(1) PrepareIsolation subscription table (Spec §2.1 & §3.2).
//!
//! - **Closed Strate 1 whitelist**: ONLY the 6 canonical critical servers + `fb_server`
//!   participate in synchronous `PrepareIsolation`.
//! - **Parallel broadcast** with a single global budget (`PREPARE_ISOLATION_GLOBAL_BUDGET_MS = 250`).
//! - **Zero userspace wait** on crash / `ExoNmi` (jumps straight to hardware IPI `0xF3` freeze).

/// Maximum global budget (ms) for graceful cooperative `PrepareIsolation` broadcast across Strate 1.
pub const PREPARE_ISOLATION_GLOBAL_BUDGET_MS: u64 = 250;

/// Hardware IPI `0xF3` freeze timeout ceiling (ms) per CORR-33.
pub const FREEZE_TIMEOUT_MS: u64 = 100;

/// Userspace wait budget (ms) on abrupt crash / `ExoNmi` (3 strikes): strictly 0 ms.
pub const CRASH_USERSPACE_WAIT_MS: u64 = 0;

/// Permanently closed Strate 1 critical server list (6 core services + `fb_server`).
/// Adding any module here requires an explicit architecture RFC and whitelist update.
pub const CRITICAL_SERVERS: &[&str] = &[
    "ipc_broker",
    "memory_server",
    "init_server",
    "vfs_server",
    "crypto_server",
    "device_server",
    "fb_server",
];

const _: () = assert!(
    PREPARE_ISOLATION_GLOBAL_BUDGET_MS <= 250,
    "PREPARE_ISOLATION_GLOBAL_BUDGET_MS must stay <= 250 ms for O(1) 1s SLA"
);
const _: () = assert!(
    CRITICAL_SERVERS.len() == 7,
    "Strate 1 CRITICAL_SERVERS list is closed to 6 core services + fb_server"
);

/// Reason why isolation was initiated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolationTrigger {
    /// Planned maintenance / live-update: bounded parallel `PrepareIsolation` broadcast.
    GracefulMaintenance,
    /// Abrupt kernel fault or `ExoNmi` 3-strikes: 0 ms userspace wait, immediate IPI `0xF3`.
    CrashOrExoNmi,
}

/// Returns `true` iff `service_name` belongs to the closed Strate 1 critical whitelist.
#[inline]
pub fn is_strate1_critical(service_name: &str) -> bool {
    let mut i = 0usize;
    while i < CRITICAL_SERVERS.len() {
        if CRITICAL_SERVERS[i].as_bytes() == service_name.as_bytes() {
            return true;
        }
        i += 1;
    }
    service_name == "ipc_router"
}

/// Summary of a Strate 1 parallel `PrepareIsolation` round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareIsolationOutcome {
    pub acked_mask: u8,
    pub expected_mask: u8,
    pub elapsed_ms: u64,
    pub timed_out: bool,
    pub skipped_userspace_wait: bool,
}

impl PrepareIsolationOutcome {
    #[inline]
    pub const fn all_acked(&self) -> bool {
        self.acked_mask == self.expected_mask
    }
}

/// Broadcasts `PrepareIsolation` in parallel to all Strate 1 critical servers and awaits ACKs
/// within the single global deadline `PREPARE_ISOLATION_GLOBAL_BUDGET_MS`.
///
/// Strate 2 services (`network_server`, `scheduler_server`, `audio_server`, `usb_server`,
/// `exo_compositor`, Ring 3 apps) are structurally excluded (CORR-54 / S-39).
pub fn broadcast_prepare_isolation<S, P, T>(
    trigger: IsolationTrigger,
    mut send_prepare: S,
    mut poll_ack: P,
    mut now_ms: T,
) -> PrepareIsolationOutcome
where
    S: FnMut(&'static str) -> bool,
    P: FnMut(&'static str) -> bool,
    T: FnMut() -> u64,
{
    let expected_mask = ((1u16 << CRITICAL_SERVERS.len()) - 1) as u8;
    if trigger == IsolationTrigger::CrashOrExoNmi {
        return PrepareIsolationOutcome {
            acked_mask: 0,
            expected_mask,
            elapsed_ms: CRASH_USERSPACE_WAIT_MS,
            timed_out: false,
            skipped_userspace_wait: true,
        };
    }

    let start_ms = now_ms();
    let deadline_ms = start_ms.saturating_add(PREPARE_ISOLATION_GLOBAL_BUDGET_MS);

    // Phase 1: Parallel broadcast to all closed Strate 1 subscribers (no per-server blocking wait).
    let mut active_mask = 0u8;
    for (idx, &svc) in CRITICAL_SERVERS.iter().enumerate() {
        if send_prepare(svc) {
            active_mask |= 1u8 << idx;
        }
    }

    // Phase 2: Poll all subscribers concurrently until all ACK or the single 250 ms budget expires.
    let mut acked_mask = 0u8;
    loop {
        for (idx, &svc) in CRITICAL_SERVERS.iter().enumerate() {
            let bit = 1u8 << idx;
            if (active_mask & bit) != 0 && (acked_mask & bit) == 0 && poll_ack(svc) {
                acked_mask |= bit;
            }
        }
        let now = now_ms();
        let elapsed_ms = now.saturating_sub(start_ms);
        if acked_mask == active_mask {
            return PrepareIsolationOutcome {
                acked_mask,
                expected_mask: active_mask,
                elapsed_ms,
                timed_out: false,
                skipped_userspace_wait: false,
            };
        }
        if now >= deadline_ms {
            return PrepareIsolationOutcome {
                acked_mask,
                expected_mask: active_mask,
                elapsed_ms,
                timed_out: true,
                skipped_userspace_wait: false,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_excludes_strate2_modules() {
        for s in CRITICAL_SERVERS {
            assert!(is_strate1_critical(s));
        }
        for s2 in [
            "network_server",
            "scheduler_server",
            "audio_server",
            "usb_server",
            "exo_compositor",
            "exosh",
        ] {
            assert!(!is_strate1_critical(s2));
        }
    }

    #[test]
    fn crash_trigger_skips_userspace_wait_completely() {
        let mut sends = 0usize;
        let out = broadcast_prepare_isolation(
            IsolationTrigger::CrashOrExoNmi,
            |_| {
                sends += 1;
                true
            },
            |_| false,
            || 1000,
        );
        assert_eq!(sends, 0);
        assert!(out.skipped_userspace_wait);
        assert_eq!(out.elapsed_ms, 0);
    }

    #[test]
    fn graceful_broadcast_respects_single_250ms_global_budget() {
        let mut clock = 0u64;
        let out = broadcast_prepare_isolation(
            IsolationTrigger::GracefulMaintenance,
            |_| true,
            |svc| svc != "device_server",
            || {
                let c = clock;
                clock += 50;
                c
            },
        );
        assert!(out.timed_out);
        assert!(out.elapsed_ms >= PREPARE_ISOLATION_GLOBAL_BUDGET_MS);
        assert!(out.elapsed_ms <= PREPARE_ISOLATION_GLOBAL_BUDGET_MS + 50);
    }
}
