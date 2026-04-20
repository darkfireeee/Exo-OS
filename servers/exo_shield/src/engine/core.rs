//! # ExoShield Core — Threat Detection Engine
//!
//! Central threat assessment, scoring, and record management.
//! All state is held in static arrays (no heap).

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

// ── Threat Level ────────────────────────────────────────────────────────────

/// Severity classification for detected threats.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ThreatLevel {
    Low      = 0,
    Medium   = 1,
    High     = 2,
    Critical = 3,
}

impl ThreatLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => ThreatLevel::Low,
            1 => ThreatLevel::Medium,
            2 => ThreatLevel::High,
            3 => ThreatLevel::Critical,
            _ => ThreatLevel::Low,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// ── Threat Category ─────────────────────────────────────────────────────────

/// Classification of the threat source / type.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreatCategory {
    None           = 0,
    Malware        = 1,
    Anomaly        = 2,
    PolicyViol     = 3,
    ResourceAbuse  = 4,
    Intrusion      = 5,
    DataExfil      = 6,
    PrivilegeEsc   = 7,
    SandboxEscape  = 8,
}

impl ThreatCategory {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => ThreatCategory::Malware,
            2 => ThreatCategory::Anomaly,
            3 => ThreatCategory::PolicyViol,
            4 => ThreatCategory::ResourceAbuse,
            5 => ThreatCategory::Intrusion,
            6 => ThreatCategory::DataExfil,
            7 => ThreatCategory::PrivilegeEsc,
            8 => ThreatCategory::SandboxEscape,
            _ => ThreatCategory::None,
        }
    }
}

// ── Threat Record ───────────────────────────────────────────────────────────

/// Maximum threat records stored simultaneously.
pub const MAX_THREAT_RECORDS: usize = 256;

/// Maximum signature name length.
pub const MAX_SIG_NAME: usize = 32;

/// A recorded threat detected by the engine.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ThreatRecord {
    pub id:         u32,
    pub pid:        u32,
    pub level:      ThreatLevel,
    pub category:   ThreatCategory,
    pub score:      u32,       // 0..1000 composite score
    pub timestamp:  u64,
    pub sig_name:   [u8; MAX_SIG_NAME],
    pub sig_len:    u8,
    pub contained:  bool,
    pub resolved:   bool,
}

impl ThreatRecord {
    pub const fn empty() -> Self {
        ThreatRecord {
            id:        0,
            pid:       0,
            level:     ThreatLevel::Low,
            category:  ThreatCategory::None,
            score:     0,
            timestamp: 0,
            sig_name:  [0u8; MAX_SIG_NAME],
            sig_len:   0,
            contained: false,
            resolved:  false,
        }
    }

    pub fn sig_name_str(&self) -> &[u8] {
        let len = self.sig_len as usize;
        if len > MAX_SIG_NAME {
            &self.sig_name[..MAX_SIG_NAME]
        } else {
            &self.sig_name[..len]
        }
    }
}

// ── Containment Action ──────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContainmentAction {
    None       = 0,
    Monitor    = 1,
    Throttle   = 2,
    Quarantine = 3,
    Kill       = 4,
}

impl ContainmentAction {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => ContainmentAction::Monitor,
            2 => ContainmentAction::Throttle,
            3 => ContainmentAction::Quarantine,
            4 => ContainmentAction::Kill,
            _ => ContainmentAction::None,
        }
    }
}

// ── Scoring System ──────────────────────────────────────────────────────────

/// Score thresholds mapping to threat levels.
const SCORE_LOW:      u32 = 0;
const SCORE_MEDIUM:   u32 = 250;
const SCORE_HIGH:     u32 = 500;
const SCORE_CRITICAL: u32 = 750;

/// Threat score weights for different factors.
const WEIGHT_SIGNATURE:   u32 = 350;
const WEIGHT_BEHAVIOR:    u32 = 250;
const WEIGHT_FREQUENCY:   u32 = 150;
const WEIGHT_SCOPE:       u32 = 150;
const WEIGHT_RECENCY:     u32 = 100;

/// Score a threat based on multiple contributing factors.
/// Returns a composite score in range 0..1000.
pub fn compute_threat_score(
    sig_match: u32,   // 0..1000 signature match confidence
    behavior:  u32,   // 0..1000 behavioral anomaly score
    frequency: u32,   // 0..1000 event frequency score
    scope:     u32,   // 0..1000 scope of impact score
    recency:   u32,   // 0..1000 recency score
) -> u32 {
    let sig_c   = sig_match.min(1000);
    let beh_c   = behavior.min(1000);
    let freq_c  = frequency.min(1000);
    let scope_c = scope.min(1000);
    let rec_c   = recency.min(1000);

    // Weighted sum with overflow-safe arithmetic
    let total = (sig_c   / 1000 * WEIGHT_SIGNATURE)
              + (beh_c   / 1000 * WEIGHT_BEHAVIOR)
              + (freq_c  / 1000 * WEIGHT_FREQUENCY)
              + (scope_c / 1000 * WEIGHT_SCOPE)
              + (rec_c   / 1000 * WEIGHT_RECENCY);

    total.min(1000)
}

/// Map a composite score to a ThreatLevel.
pub fn score_to_level(score: u32) -> ThreatLevel {
    if score >= SCORE_CRITICAL {
        ThreatLevel::Critical
    } else if score >= SCORE_HIGH {
        ThreatLevel::High
    } else if score >= SCORE_MEDIUM {
        ThreatLevel::Medium
    } else {
        ThreatLevel::Low
    }
}

/// Determine the containment action based on threat level.
pub fn containment_for_level(level: ThreatLevel) -> ContainmentAction {
    match level {
        ThreatLevel::Low      => ContainmentAction::Monitor,
        ThreatLevel::Medium   => ContainmentAction::Throttle,
        ThreatLevel::High     => ContainmentAction::Quarantine,
        ThreatLevel::Critical => ContainmentAction::Kill,
    }
}

// ── Global Threat Store ─────────────────────────────────────────────────────

static THREAT_STORE: Mutex<ThreatStore> = Mutex::new(ThreatStore::new());

struct ThreatStore {
    records:  [ThreatRecord; MAX_THREAT_RECORDS],
    count:    u32,
    next_id:  u32,
}

impl ThreatStore {
    const fn new() -> Self {
        ThreatStore {
            records:  [ThreatRecord::empty(); MAX_THREAT_RECORDS],
            count:    0,
            next_id:  1,
        }
    }

    fn insert(&mut self, rec: &ThreatRecord) -> Option<u32> {
        // Find an empty slot (resolved or id==0)
        for i in 0..MAX_THREAT_RECORDS {
            if self.records[i].id == 0 || self.records[i].resolved {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                let mut new_rec = *rec;
                new_rec.id = id;
                self.records[i] = new_rec;
                self.count = self.count.saturating_add(1).min(MAX_THREAT_RECORDS as u32);
                return Some(id);
            }
        }
        // Table full — overwrite the oldest resolved entry by scanning
        let mut oldest_idx = 0usize;
        let mut oldest_ts = u64::MAX;
        for i in 0..MAX_THREAT_RECORDS {
            if self.records[i].resolved && self.records[i].timestamp < oldest_ts {
                oldest_ts = self.records[i].timestamp;
                oldest_idx = i;
            }
        }
        if oldest_ts != u64::MAX {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            let mut new_rec = *rec;
            new_rec.id = id;
            self.records[oldest_idx] = new_rec;
            return Some(id);
        }
        None
    }

    fn get_by_id(&self, id: u32) -> Option<ThreatRecord> {
        for i in 0..MAX_THREAT_RECORDS {
            if self.records[i].id == id {
                return Some(self.records[i]);
            }
        }
        None
    }

    fn get_by_pid(&self, pid: u32, out: &mut [ThreatRecord], max: usize) -> usize {
        let mut written = 0usize;
        for i in 0..MAX_THREAT_RECORDS {
            if written >= max { break; }
            if self.records[i].id != 0 && self.records[i].pid == pid {
                out[written] = self.records[i];
                written += 1;
            }
        }
        written
    }

    fn mark_contained(&mut self, id: u32) -> bool {
        for i in 0..MAX_THREAT_RECORDS {
            if self.records[i].id == id {
                self.records[i].contained = true;
                return true;
            }
        }
        false
    }

    fn mark_resolved(&mut self, id: u32) -> bool {
        for i in 0..MAX_THREAT_RECORDS {
            if self.records[i].id == id {
                self.records[i].resolved = true;
                return true;
            }
        }
        false
    }

    fn count_by_level(&self, level: ThreatLevel) -> u32 {
        let mut c = 0u32;
        for i in 0..MAX_THREAT_RECORDS {
            if self.records[i].id != 0
                && !self.records[i].resolved
                && self.records[i].level == level
            {
                c += 1;
            }
        }
        c
    }

    fn active_count(&self) -> u32 {
        let mut c = 0u32;
        for i in 0..MAX_THREAT_RECORDS {
            if self.records[i].id != 0 && !self.records[i].resolved {
                c += 1;
            }
        }
        c
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Record a new threat. Returns the threat ID on success.
pub fn record_threat(rec: &ThreatRecord) -> Option<u32> {
    let mut store = THREAT_STORE.lock();
    store.insert(rec)
}

/// Look up a threat by ID.
pub fn get_threat(id: u32) -> Option<ThreatRecord> {
    let store = THREAT_STORE.lock();
    store.get_by_id(id)
}

/// Look up threats by PID. Writes up to `max` results into `out`.
/// Returns the number of records written.
pub fn get_threats_by_pid(pid: u32, out: &mut [ThreatRecord], max: usize) -> usize {
    let store = THREAT_STORE.lock();
    store.get_by_pid(pid, out, max)
}

/// Mark a threat as contained (quarantined).
pub fn contain_threat(id: u32) -> bool {
    let mut store = THREAT_STORE.lock();
    store.mark_contained(id)
}

/// Mark a threat as resolved.
pub fn resolve_threat(id: u32) -> bool {
    let mut store = THREAT_STORE.lock();
    store.mark_resolved(id)
}

/// Count active (unresolved) threats at a given level.
pub fn count_threats_at_level(level: ThreatLevel) -> u32 {
    let store = THREAT_STORE.lock();
    store.count_by_level(level)
}

/// Count all active (unresolved) threats.
pub fn active_threat_count() -> u32 {
    let store = THREAT_STORE.lock();
    store.active_count()
}

// ── Threat Assessment ───────────────────────────────────────────────────────

/// Result of a full threat assessment for a process.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ThreatAssessment {
    pub pid:               u32,
    pub composite_score:   u32,
    pub level:             ThreatLevel,
    pub recommended_action: ContainmentAction,
    pub active_threats:    u32,
    pub history_threats:   u32,
}

/// Perform a threat assessment for a given PID.
/// Aggregates all threat records for the process and computes
/// a composite assessment.
pub fn assess_pid(pid: u32, current_tick: u64) -> ThreatAssessment {
    let mut threats = [ThreatRecord::empty(); 32];
    let n = get_threats_by_pid(pid, &mut threats, 32);

    let mut max_score: u32 = 0;
    let mut max_level = ThreatLevel::Low;
    let mut active = 0u32;
    let mut history = 0u32;
    let mut sig_agg = 0u32;
    let mut beh_agg = 0u32;

    for i in 0..n {
        let rec = &threats[i];
        if !rec.resolved {
            active += 1;
            // Aggregate scores: take the max and add decayed contributions
            if rec.score > max_score {
                max_score = rec.score;
            }
            if rec.level > max_level {
                max_level = rec.level;
            }
            // Accumulate partial scores from recency-weighted contributions
            let age = current_tick.saturating_sub(rec.timestamp);
            let recency_factor = if age < 100 { 1000u32 } else if age < 1000 { 500 } else { 100 };
            sig_agg += (rec.score / 4).min(250) * recency_factor / 1000;
            beh_agg += (rec.score / 4).min(250) * recency_factor / 1000;
        }
        history += 1;
    }

    // If active threats, recompute score with aggregated data
    let composite = if active > 0 {
        let freq_score = if active > 5 { 800 } else if active > 2 { 500 } else { 200 };
        let scope_score = if history > 10 { 700 } else if history > 3 { 400 } else { 150 };
        compute_threat_score(
            sig_agg.min(1000),
            beh_agg.min(1000),
            freq_score,
            scope_score,
            800, // high recency — we're assessing now
        )
    } else {
        max_score
    };

    let level = score_to_level(composite);
    let action = containment_for_level(level);

    ThreatAssessment {
        pid:                pid,
        composite_score:    composite,
        level:              level,
        recommended_action: action,
        active_threats:     active,
        history_threats:    history,
    }
}

// ── Process Risk Profile ────────────────────────────────────────────────────

/// Maximum tracked processes.
pub const MAX_TRACKED_PROCESSES: usize = 128;

/// Per-process risk profile maintained by the core engine.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProcessRiskProfile {
    pub pid:              u32,
    pub baseline_score:   u32,    // score at last assessment
    pub peak_score:       u32,    // highest score observed
    pub syscall_rate:     u32,    // syscalls per tick window
    pub net_rate:         u32,    // network ops per tick window
    pub fs_rate:          u32,    // filesystem ops per tick window
    pub anomaly_count:    u32,    // cumulative anomaly hits
    pub last_assess_tick: u64,
    pub contained:        bool,
    pub quarantine_tick:  u64,
}

impl ProcessRiskProfile {
    pub const fn empty() -> Self {
        ProcessRiskProfile {
            pid:              0,
            baseline_score:   0,
            peak_score:       0,
            syscall_rate:     0,
            net_rate:         0,
            fs_rate:          0,
            anomaly_count:    0,
            last_assess_tick: 0,
            contained:        false,
            quarantine_tick:  0,
        }
    }
}

static RISK_PROFILES: Mutex<RiskProfileStore> = Mutex::new(RiskProfileStore::new());

struct RiskProfileStore {
    profiles: [ProcessRiskProfile; MAX_TRACKED_PROCESSES],
    count:    u32,
}

impl RiskProfileStore {
    const fn new() -> Self {
        RiskProfileStore {
            profiles: [ProcessRiskProfile::empty(); MAX_TRACKED_PROCESSES],
            count:    0,
        }
    }

    fn get_or_create(&mut self, pid: u32) -> Option<&mut ProcessRiskProfile> {
        // Find existing
        for i in 0..MAX_TRACKED_PROCESSES {
            if self.profiles[i].pid == pid {
                return Some(&mut self.profiles[i]);
            }
        }
        // Find empty slot
        for i in 0..MAX_TRACKED_PROCESSES {
            if self.profiles[i].pid == 0 {
                self.profiles[i] = ProcessRiskProfile {
                    pid: pid,
                    baseline_score: 0,
                    peak_score: 0,
                    syscall_rate: 0,
                    net_rate: 0,
                    fs_rate: 0,
                    anomaly_count: 0,
                    last_assess_tick: 0,
                    contained: false,
                    quarantine_tick: 0,
                };
                self.count = self.count.saturating_add(1).min(MAX_TRACKED_PROCESSES as u32);
                return Some(&mut self.profiles[i]);
            }
        }
        None
    }

    fn get(&self, pid: u32) -> Option<ProcessRiskProfile> {
        for i in 0..MAX_TRACKED_PROCESSES {
            if self.profiles[i].pid == pid {
                return Some(self.profiles[i]);
            }
        }
        None
    }
}

/// Update a process risk profile with new measurement data.
pub fn update_risk_profile(
    pid: u32,
    syscall_rate: u32,
    net_rate: u32,
    fs_rate: u32,
    anomaly_delta: u32,
    tick: u64,
) -> bool {
    let mut store = RISK_PROFILES.lock();
    if let Some(profile) = store.get_or_create(pid) {
        profile.syscall_rate = syscall_rate;
        profile.net_rate = net_rate;
        profile.fs_rate = fs_rate;
        profile.anomaly_count = profile.anomaly_count.saturating_add(anomaly_delta);
        profile.last_assess_tick = tick;
        // Update peak score if current assessment is higher
        let total_rate = syscall_rate.saturating_add(net_rate).saturating_add(fs_rate);
        let rate_score = total_rate.min(1000);
        if rate_score > profile.peak_score {
            profile.peak_score = rate_score;
        }
        profile.baseline_score = rate_score;
        return true;
    }
    false
}

/// Retrieve a copy of a process risk profile.
pub fn get_risk_profile(pid: u32) -> Option<ProcessRiskProfile> {
    let store = RISK_PROFILES.lock();
    store.get(pid)
}

/// Mark a process as contained in its risk profile.
pub fn mark_process_contained(pid: u32, tick: u64) -> bool {
    let mut store = RISK_PROFILES.lock();
    if let Some(profile) = store.get_or_create(pid) {
        profile.contained = true;
        profile.quarantine_tick = tick;
        return true;
    }
    false
}

/// Release a process from containment.
pub fn release_process(pid: u32) -> bool {
    let mut store = RISK_PROFILES.lock();
    if let Some(profile) = store.get_or_create(pid) {
        profile.contained = false;
        profile.quarantine_tick = 0;
        return true;
    }
    false
}

// ── Statistics ──────────────────────────────────────────────────────────────

static STATS_TOTAL_ASSESSMENTS: AtomicU64 = AtomicU64::new(0);
static STATS_TOTAL_THREATS:     AtomicU64 = AtomicU64::new(0);
static STATS_CONTAINMENTS:      AtomicU64 = AtomicU64::new(0);
static STATS_RESOLVED:          AtomicU64 = AtomicU64::new(0);
static STATS_CRITICAL_ALERTS:   AtomicU32 = AtomicU32::new(0);
static STATS_ENGINE_INIT:       AtomicBool = AtomicBool::new(false);

/// Initialize the core engine.
pub fn core_init() {
    // Clear all threat records
    {
        let mut store = THREAT_STORE.lock();
        for i in 0..MAX_THREAT_RECORDS {
            store.records[i] = ThreatRecord::empty();
        }
        store.count = 0;
        store.next_id = 1;
    }
    // Clear all risk profiles
    {
        let mut profiles = RISK_PROFILES.lock();
        for i in 0..MAX_TRACKED_PROCESSES {
            profiles.profiles[i] = ProcessRiskProfile::empty();
        }
        profiles.count = 0;
    }
    STATS_ENGINE_INIT.store(true, Ordering::Release);
}

/// Check if the core engine has been initialized.
pub fn core_is_init() -> bool {
    STATS_ENGINE_INIT.load(Ordering::Acquire)
}

/// Increment assessment counter and return previous value.
pub fn stat_assessments_inc() -> u64 {
    STATS_TOTAL_ASSESSMENTS.fetch_add(1, Ordering::Relaxed)
}

/// Increment threats counter.
pub fn stat_threats_inc() {
    STATS_TOTAL_THREATS.fetch_add(1, Ordering::Relaxed);
}

/// Increment containments counter.
pub fn stat_containments_inc() {
    STATS_CONTAINMENTS.fetch_add(1, Ordering::Relaxed);
}

/// Increment resolved counter.
pub fn stat_resolved_inc() {
    STATS_RESOLVED.fetch_add(1, Ordering::Relaxed);
}

/// Increment critical alerts counter.
pub fn stat_critical_inc() {
    STATS_CRITICAL_ALERTS.fetch_add(1, Ordering::Relaxed);
}

/// Get engine statistics as a packed struct.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CoreStats {
    pub total_assessments: u64,
    pub total_threats:     u64,
    pub containments:      u64,
    pub resolved:          u64,
    pub critical_alerts:   u32,
    pub active_threats:    u32,
}

/// Retrieve current engine statistics.
pub fn get_core_stats() -> CoreStats {
    CoreStats {
        total_assessments: STATS_TOTAL_ASSESSMENTS.load(Ordering::Relaxed),
        total_threats:     STATS_TOTAL_THREATS.load(Ordering::Relaxed),
        containments:      STATS_CONTAINMENTS.load(Ordering::Relaxed),
        resolved:          STATS_RESOLVED.load(Ordering::Relaxed),
        critical_alerts:   STATS_CRITICAL_ALERTS.load(Ordering::Relaxed),
        active_threats:    active_threat_count(),
    }
}
