// kernel/src/memory/utils/oom_killer.rs
//
// OOM Killer — victim selection and signaling upon memory exhaustion.
//
// Architecture:
//   • OOM killer is triggered by `swap/policy.rs` (is_critical) or by
//     the buddy allocator when all zones are exhausted.
//   • It selects a victim via an `OomScorer` trait (dependency inversion).
//   • It signals termination via a fn pointer (OOM_KILL_SENDER) registered by
//     process/ — layer 0 cannot call process/ directly.
//
// LAYER 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Trait OomScorer — provided by process/
// ─────────────────────────────────────────────────────────────────────────────

/// Information about an OOM elimination candidate.
#[derive(Debug, Clone, Copy)]
pub struct OomKillCandidate {
    /// Process ID.
    pub pid: u64,
    /// Calculated OOM score (higher = higher priority to kill).
    pub oom_score: u64,
    /// Resident RSS in pages.
    pub vm_rss: u64,
    /// Process name (for logging).
    pub name: [u8; 16],
}

impl OomKillCandidate {
    pub const fn invalid() -> Self {
        Self {
            pid: 0,
            oom_score: 0,
            vm_rss: 0,
            name: [0; 16],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.pid != 0
    }
}

/// Trait that process/ implements to provide list of OOM candidates.
pub trait OomScorer {
    /// Returns most appropriate candidate to kill.
    fn pick_victim(&self) -> Option<OomKillCandidate>;
    /// Returns slice of all candidates sorted by descending score.
    fn candidates(&self, buf: &mut [OomKillCandidate]) -> usize;
}

// ─────────────────────────────────────────────────────────────────────────────
// Default scorer (RSS-based) before process/ is initialized
// ─────────────────────────────────────────────────────────────────────────────

pub struct DefaultOomScorer;

impl OomScorer for DefaultOomScorer {
    fn pick_victim(&self) -> Option<OomKillCandidate> {
        None
    }
    fn candidates(&self, buf: &mut [OomKillCandidate]) -> usize {
        fill_registered_candidates(buf)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Signaling fn pointer -> process/
// ─────────────────────────────────────────────────────────────────────────────

/// Prototype of OOM signaling function registered by process/.
/// `pid`: PID to kill.
/// Returns `true` if signal could be sent.
pub type OomKillSendFn = fn(pid: u64) -> bool;
pub type OomCandidateProviderFn = fn(buf: &mut [OomKillCandidate]) -> usize;

#[allow(dead_code)]
fn nop_oom_kill(_pid: u64) -> bool {
    false
}

/// Function pointer to process/ OOM handler — write-once.
static OOM_KILL_SENDER: AtomicUsize = AtomicUsize::new(0);
/// Function pointer to process/ OOM candidate provider.
static OOM_CANDIDATE_PROVIDER: AtomicUsize = AtomicUsize::new(0);

/// Registers process/ OOM handler.
/// Must be called once by process/ during its init.
pub fn register_oom_kill_sender(f: OomKillSendFn) {
    OOM_KILL_SENDER
        .compare_exchange(0, f as usize, Ordering::Release, Ordering::Relaxed)
        .ok();
}

/// Registers process/ OOM candidate provider.
/// Must be called once by process/ during its init.
pub fn register_oom_candidate_provider(f: OomCandidateProviderFn) {
    OOM_CANDIDATE_PROVIDER
        .compare_exchange(0, f as usize, Ordering::Release, Ordering::Relaxed)
        .ok();
}

/// Calls registered OOM sender.
fn invoke_kill_sender(pid: u64) -> bool {
    let ptr = OOM_KILL_SENDER.load(Ordering::Acquire);
    if ptr == 0 {
        return false;
    }
    let f: OomKillSendFn = unsafe { core::mem::transmute(ptr) };
    f(pid)
}

fn fill_registered_candidates(buf: &mut [OomKillCandidate]) -> usize {
    let ptr = OOM_CANDIDATE_PROVIDER.load(Ordering::Acquire);
    if ptr == 0 {
        return 0;
    }
    let f: OomCandidateProviderFn = unsafe { core::mem::transmute(ptr) };
    f(buf).min(buf.len())
}

// ─────────────────────────────────────────────────────────────────────────────
// OOM Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct OomStats {
    pub oom_invocations: AtomicU64,
    pub victims_selected: AtomicU64,
    pub kills_sent: AtomicU64,
    pub kills_failed: AtomicU64,
    /// Triggers in critical situations (empty zone).
    pub critical_events: AtomicU64,
    /// Ignore OOM: flag set when kernel is shutting down.
    pub suppressed: AtomicBool,
}

impl Default for OomStats {
    fn default() -> Self {
        Self::new()
    }
}

impl OomStats {
    const fn new() -> Self {
        Self {
            oom_invocations: AtomicU64::new(0),
            victims_selected: AtomicU64::new(0),
            kills_sent: AtomicU64::new(0),
            kills_failed: AtomicU64::new(0),
            critical_events: AtomicU64::new(0),
            suppressed: AtomicBool::new(false),
        }
    }
}

unsafe impl Sync for OomStats {}
pub static OOM_STATS: OomStats = OomStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Cooldown — prevent kill storms
// ─────────────────────────────────────────────────────────────────────────────

/// TSC of last kill. Prevents a second kill within next 100 ms.
static LAST_KILL_TSC: AtomicU64 = AtomicU64::new(0);
/// TSC frequency in Hz (estimated at init, 3 GHz default).
static TSC_HZ: AtomicU64 = AtomicU64::new(3_000_000_000);

/// Configures TSC frequency (called by arch/ after calibration).
pub fn set_tsc_hz(hz: u64) {
    TSC_HZ.store(hz, Ordering::Relaxed);
}

#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdtsc available on x86_64; non-serializing sufficient for OOM timestamp.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Returns `true` if enough time has elapsed since last OOM kill.
fn cooldown_elapsed() -> bool {
    let now = rdtsc();
    let last = LAST_KILL_TSC.load(Ordering::Relaxed);
    if last == 0 {
        return true;
    }
    let elapsed_ticks = now.wrapping_sub(last);
    let hz = TSC_HZ.load(Ordering::Relaxed);
    // 100 ms cooldown.
    elapsed_ticks >= hz / 10
}

// ─────────────────────────────────────────────────────────────────────────────
// Victim selection
// ─────────────────────────────────────────────────────────────────────────────

/// Static buffer for OOM candidates (avoids any allocation on emergency path).
static OOM_CANDIDATE_BUF: Mutex<[OomKillCandidate; 64]> = Mutex::new(
    [OomKillCandidate {
        pid: 0,
        oom_score: 0,
        vm_rss: 0,
        name: [0; 16],
    }; 64],
);

/// Selects OOM victim via provided scorer.
///
/// Returns `Some(pid)` or `None` (no candidate or active cooldown).
pub fn select_oom_victim<S: OomScorer>(scorer: &S) -> Option<u64> {
    if OOM_STATS.suppressed.load(Ordering::Acquire) {
        return None;
    }
    if !cooldown_elapsed() {
        return None;
    }

    OOM_STATS.oom_invocations.fetch_add(1, Ordering::Relaxed);

    // Try `pick_victim` first (O(1) if scorer supports it).
    if let Some(victim) = scorer.pick_victim() {
        if victim.is_valid() {
            OOM_STATS.victims_selected.fetch_add(1, Ordering::Relaxed);
            return Some(victim.pid);
        }
    }

    // Fallback: get list and take first.
    let mut buf = OOM_CANDIDATE_BUF.lock();
    let n = scorer.candidates(&mut *buf);
    if n == 0 {
        return None;
    }
    // Sort by descending score (simple, n <= 64).
    let slice = &mut buf[..n];
    slice.sort_unstable_by_key(|b| core::cmp::Reverse(b.oom_score));
    let pid = slice[0].pid;
    OOM_STATS.victims_selected.fetch_add(1, Ordering::Relaxed);
    Some(pid)
}

// ─────────────────────────────────────────────────────────────────────────────
// OOM entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Triggers OOM killer with `scorer` as candidate source.
///
/// Flow:
///   1. select_oom_victim(scorer)
///   2. invoke_kill_sender(pid)
///   3. Update LAST_KILL_TSC
///
/// Returns `true` if a kill was sent.
pub fn oom_kill<S: OomScorer>(scorer: &S, critical: bool) -> bool {
    if critical {
        OOM_STATS.critical_events.fetch_add(1, Ordering::Relaxed);
    }

    let Some(pid) = select_oom_victim(scorer) else {
        return false;
    };

    let sent = invoke_kill_sender(pid);
    if sent {
        LAST_KILL_TSC.store(rdtsc(), Ordering::Relaxed);
        OOM_STATS.kills_sent.fetch_add(1, Ordering::Relaxed);
    } else {
        OOM_STATS.kills_failed.fetch_add(1, Ordering::Relaxed);
    }
    sent
}

/// Suppresses OOM triggers (e.g. shutdown, recover).
pub fn oom_suppress() {
    OOM_STATS.suppressed.store(true, Ordering::Release);
}

/// Re-enables OOM killer.
pub fn oom_unsuppress() {
    OOM_STATS.suppressed.store(false, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Global OOM killer with default scorer
// ─────────────────────────────────────────────────────────────────────────────

/// Triggers OOM killer with default scorer (before process/ is initialized).
pub fn oom_kill_default() -> bool {
    let scorer = DefaultOomScorer;
    oom_kill(&scorer, false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // Nothing to initialize; structures declared const.
}
