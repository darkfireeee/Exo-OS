//! # ExoShield Scanner — Signature & Heuristic Scanner
//!
//! Provides signature-based and heuristic-based scanning of processes
//! and memory regions. Includes a scan queue (max 64), scan profiles,
//! and a periodic scan scheduler.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

use super::core::{
    ThreatLevel, ThreatCategory, ThreatRecord,
    MAX_SIG_NAME, record_threat, score_to_level,
    stat_threats_inc, stat_critical_inc,
};

// ── Constants ───────────────────────────────────────────────────────────────

/// Maximum entries in the scan queue.
pub const SCAN_QUEUE_MAX: usize = 64;

/// Maximum signatures in the built-in database.
pub const MAX_SIGNATURES: usize = 128;

/// Maximum scan profile name length.
pub const MAX_PROFILE_NAME: usize = 24;

/// Scan interval in ticks for periodic scans.
pub const DEFAULT_SCAN_INTERVAL_TICKS: u64 = 1000;

// ── Signature Entry ─────────────────────────────────────────────────────────

/// A signature entry for matching against process data.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignatureEntry {
    pub id:         u32,
    pub sig_type:   u8,        // 0=pattern, 1=heuristic, 2=behavioral
    pub category:   ThreatCategory,
    pub severity:   ThreatLevel,
    pub base_score: u32,       // 0..1000
    pub pattern:    [u8; 16],  // pattern bytes for matching
    pub pattern_len: u8,
    pub name:       [u8; MAX_SIG_NAME],
    pub name_len:   u8,
    pub enabled:    bool,
}

impl SignatureEntry {
    pub const fn empty() -> Self {
        SignatureEntry {
            id:           0,
            sig_type:     0,
            category:     ThreatCategory::None,
            severity:     ThreatLevel::Low,
            base_score:   0,
            pattern:      [0u8; 16],
            pattern_len:  0,
            name:         [0u8; MAX_SIG_NAME],
            name_len:     0,
            enabled:      false,
        }
    }
}

// ── Scan Result ─────────────────────────────────────────────────────────────

/// Result of a scan operation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScanResult {
    pub scan_id:     u32,
    pub pid:         u32,
    pub matched:     bool,
    pub match_count: u32,
    pub max_severity: ThreatLevel,
    pub composite:   u32,         // aggregate score 0..1000
    pub timestamp:   u64,
    pub sig_ids:     [u32; 8],    // up to 8 matched signature IDs
    pub sig_count:   u8,
    pub status:      u8,          // 0=pending, 1=complete, 2=error
}

impl ScanResult {
    pub const fn empty() -> Self {
        ScanResult {
            scan_id:      0,
            pid:          0,
            matched:      false,
            match_count:  0,
            max_severity: ThreatLevel::Low,
            composite:    0,
            timestamp:    0,
            sig_ids:      [0u32; 8],
            sig_count:    0,
            status:       0,
        }
    }
}

// ── Scan Request (queue entry) ──────────────────────────────────────────────

/// A pending scan request in the scan queue.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScanRequest {
    pub id:         u32,
    pub pid:        u32,
    pub scan_type:  u8,    // 0=full, 1=quick, 2=memory, 3=behavioral
    pub priority:   u8,    // 0=low, 1=normal, 2=high, 3=critical
    pub submitted:  u64,
    pub started:    u64,
    pub completed:  u64,
    pub active:     bool,
}

impl ScanRequest {
    pub const fn empty() -> Self {
        ScanRequest {
            id:        0,
            pid:       0,
            scan_type: 0,
            priority:  1,
            submitted: 0,
            started:   0,
            completed: 0,
            active:    false,
        }
    }
}

// ── Scan Profile ────────────────────────────────────────────────────────────

/// A named scan profile that defines what and how to scan.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScanProfile {
    pub name:           [u8; MAX_PROFILE_NAME],
    pub name_len:       u8,
    pub scan_memory:    bool,
    pub scan_syscalls:  bool,
    pub scan_network:   bool,
    pub scan_fs:        bool,
    pub scan_behavior:  bool,
    pub max_depth:      u8,
    pub timeout_ticks:  u16,
    pub heuristic_level: u8,  // 0=off, 1=low, 2=medium, 3=aggressive
    pub enabled:        bool,
}

impl ScanProfile {
    pub const fn empty() -> Self {
        ScanProfile {
            name:            [0u8; MAX_PROFILE_NAME],
            name_len:        0,
            scan_memory:     false,
            scan_syscalls:   false,
            scan_network:    false,
            scan_fs:         false,
            scan_behavior:   false,
            max_depth:       3,
            timeout_ticks:   500,
            heuristic_level: 1,
            enabled:         false,
        }
    }
}

// ── Signature Database ──────────────────────────────────────────────────────

static SIG_DB: Mutex<SignatureDatabase> = Mutex::new(SignatureDatabase::new());

struct SignatureDatabase {
    entries: [SignatureEntry; MAX_SIGNATURES],
    count:   u32,
    next_id: u32,
}

impl SignatureDatabase {
    const fn new() -> Self {
        SignatureDatabase {
            entries: [SignatureEntry::empty(); MAX_SIGNATURES],
            count:   0,
            next_id: 1,
        }
    }

    fn insert(&mut self, entry: &SignatureEntry) -> Option<u32> {
        if self.count as usize >= MAX_SIGNATURES {
            return None;
        }
        for i in 0..MAX_SIGNATURES {
            if self.entries[i].id == 0 {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                let mut new_entry = *entry;
                new_entry.id = id;
                self.entries[i] = new_entry;
                self.count += 1;
                return Some(id);
            }
        }
        None
    }

    fn find_by_id(&self, id: u32) -> Option<SignatureEntry> {
        for i in 0..MAX_SIGNATURES {
            if self.entries[i].id == id {
                return Some(self.entries[i]);
            }
        }
        None
    }

    fn disable(&mut self, id: u32) -> bool {
        for i in 0..MAX_SIGNATURES {
            if self.entries[i].id == id {
                self.entries[i].enabled = false;
                return true;
            }
        }
        false
    }

    fn enable(&mut self, id: u32) -> bool {
        for i in 0..MAX_SIGNATURES {
            if self.entries[i].id == id {
                self.entries[i].enabled = true;
                return true;
            }
        }
        false
    }
}

// ── Scan Queue ──────────────────────────────────────────────────────────────

static SCAN_QUEUE: Mutex<ScanQueue> = Mutex::new(ScanQueue::new());

struct ScanQueue {
    requests: [ScanRequest; SCAN_QUEUE_MAX],
    head:     u32,
    tail:     u32,
    count:    u32,
    next_id:  u32,
}

impl ScanQueue {
    const fn new() -> Self {
        ScanQueue {
            requests: [ScanRequest::empty(); SCAN_QUEUE_MAX],
            head:     0,
            tail:     0,
            count:    0,
            next_id:  1,
        }
    }

    fn enqueue(&mut self, pid: u32, scan_type: u8, priority: u8, tick: u64) -> Option<u32> {
        if self.count as usize >= SCAN_QUEUE_MAX {
            // Try to reclaim completed entries
            self.reclaim_completed();
            if self.count as usize >= SCAN_QUEUE_MAX {
                return None;
            }
        }

        let idx = self.tail as usize % SCAN_QUEUE_MAX;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.requests[idx] = ScanRequest {
            id:        id,
            pid:       pid,
            scan_type: scan_type,
            priority:  priority,
            submitted: tick,
            started:   0,
            completed: 0,
            active:    false,
        };
        self.tail = self.tail.wrapping_add(1);
        self.count += 1;
        Some(id)
    }

    fn dequeue_next(&mut self) -> Option<ScanRequest> {
        if self.count == 0 {
            return None;
        }

        // Find highest-priority pending request
        let mut best_idx = usize::MAX;
        let mut best_priority = 0u8;
        let mut best_submitted = u64::MAX;

        for i in 0..SCAN_QUEUE_MAX {
            let req = &self.requests[i];
            if req.id != 0 && !req.active && req.completed == 0 {
                if req.priority > best_priority
                    || (req.priority == best_priority && req.submitted < best_submitted)
                {
                    best_priority = req.priority;
                    best_submitted = req.submitted;
                    best_idx = i;
                }
            }
        }

        if best_idx == usize::MAX {
            return None;
        }

        let req = self.requests[best_idx];
        self.requests[best_idx].active = true;
        Some(req)
    }

    fn complete(&mut self, id: u32, tick: u64) -> bool {
        for i in 0..SCAN_QUEUE_MAX {
            if self.requests[i].id == id {
                self.requests[i].active = false;
                self.requests[i].completed = tick;
                return true;
            }
        }
        false
    }

    fn reclaim_completed(&mut self) {
        for i in 0..SCAN_QUEUE_MAX {
            if self.requests[i].id != 0 && self.requests[i].completed != 0 {
                self.requests[i] = ScanRequest::empty();
                self.count = self.count.saturating_sub(1);
            }
        }
    }

    fn pending_count(&self) -> u32 {
        let mut c = 0u32;
        for i in 0..SCAN_QUEUE_MAX {
            if self.requests[i].id != 0 && self.requests[i].completed == 0 {
                c += 1;
            }
        }
        c
    }
}

// ── Scan Profiles ───────────────────────────────────────────────────────────

static SCAN_PROFILES: Mutex<ProfileStore> = Mutex::new(ProfileStore::new());

const NUM_PROFILES: usize = 8;

struct ProfileStore {
    profiles: [ScanProfile; NUM_PROFILES],
    count:    u32,
}

impl ProfileStore {
    const fn new() -> Self {
        ProfileStore {
            profiles: [ScanProfile::empty(); NUM_PROFILES],
            count:    0,
        }
    }

    fn set(&mut self, idx: usize, profile: &ScanProfile) -> bool {
        if idx >= NUM_PROFILES {
            return false;
        }
        self.profiles[idx] = *profile;
        self.profiles[idx].enabled = true;
        true
    }

    fn get(&self, idx: usize) -> Option<ScanProfile> {
        if idx >= NUM_PROFILES {
            return None;
        }
        Some(self.profiles[idx])
    }

    fn active_profile(&self) -> Option<ScanProfile> {
        for i in 0..NUM_PROFILES {
            if self.profiles[i].enabled {
                return Some(self.profiles[i]);
            }
        }
        None
    }
}

// ── Scan Results Store ──────────────────────────────────────────────────────

const MAX_RESULTS: usize = 64;

static SCAN_RESULTS: Mutex<ScanResultStore> = Mutex::new(ScanResultStore::new());

struct ScanResultStore {
    results: [ScanResult; MAX_RESULTS],
    next_id: u32,
}

impl ScanResultStore {
    const fn new() -> Self {
        ScanResultStore {
            results: [ScanResult::empty(); MAX_RESULTS],
            next_id: 1,
        }
    }

    fn store(&mut self, result: &ScanResult) -> Option<u32> {
        // Find empty slot
        for i in 0..MAX_RESULTS {
            if self.results[i].scan_id == 0 {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                let mut r = *result;
                r.scan_id = id;
                self.results[i] = r;
                return Some(id);
            }
        }
        // Overwrite oldest
        let mut oldest_idx = 0usize;
        let mut oldest_ts = u64::MAX;
        for i in 0..MAX_RESULTS {
            if self.results[i].timestamp < oldest_ts {
                oldest_ts = self.results[i].timestamp;
                oldest_idx = i;
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut r = *result;
        r.scan_id = id;
        self.results[oldest_idx] = r;
        Some(id)
    }

    fn get(&self, id: u32) -> Option<ScanResult> {
        for i in 0..MAX_RESULTS {
            if self.results[i].scan_id == id {
                return Some(self.results[i]);
            }
        }
        None
    }

    fn get_by_pid(&self, pid: u32) -> Option<ScanResult> {
        let mut best: Option<ScanResult> = None;
        let mut best_ts = 0u64;
        for i in 0..MAX_RESULTS {
            if self.results[i].pid == pid && self.results[i].timestamp > best_ts {
                best_ts = self.results[i].timestamp;
                best = Some(self.results[i]);
            }
        }
        best
    }
}

// ── Pattern Matching ────────────────────────────────────────────────────────

/// Match a byte pattern against a data buffer using simple sliding window.
/// Returns the number of matches found.
fn match_pattern(data: &[u8], pattern: &[u8]) -> u32 {
    if pattern.len() == 0 || data.len() < pattern.len() {
        return 0;
    }
    let mut count = 0u32;
    let mut i = 0usize;
    while i + pattern.len() <= data.len() {
        let mut matched = true;
        for j in 0..pattern.len() {
            if data[i + j] != pattern[j] {
                matched = false;
                break;
            }
        }
        if matched {
            count += 1;
            i += pattern.len(); // skip past match
        } else {
            i += 1;
        }
    }
    count
}

/// Compute a FNV-1a hash of a byte slice for heuristic matching.
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut h: u32 = 2166136261;
    for &b in data.iter() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

// ── Heuristic Analysis ──────────────────────────────────────────────────────

/// Entropy calculation for heuristic analysis.
/// Returns a value 0..1000 representing byte entropy.
pub fn compute_entropy(data: &[u8]) -> u32 {
    if data.len() == 0 {
        return 0;
    }
    let mut freq = [0u32; 256];
    for &b in data.iter() {
        freq[b as usize] += 1;
    }
    let len = data.len() as u32;
    let mut entropy = 0u32;
    for i in 0..256 {
        if freq[i] > 0 {
            let p = (freq[i] * 1000) / len;
            // Approximate -p*log2(p) * 1000 using a lookup approach
            // log2(x/1000) ~ simplified computation
            if p > 0 && p < 1000 {
                // Approximate: -p * log2(p/1000) * 1000
                // Use a simple piecewise linear approximation
                let contrib = if p > 500 { 1000 - p }
                              else if p > 250 { p }
                              else if p > 100 { p + 200 }
                              else { p + 400 };
                entropy += contrib.min(800);
            }
        }
    }
    // Normalize: max entropy is ~8000 for uniform distribution over 256 bytes
    entropy.min(1000)
}

/// Analyze a memory region heuristically.
/// Returns a heuristic score 0..1000.
pub fn heuristic_analyze(data: &[u8]) -> u32 {
    if data.len() == 0 {
        return 0;
    }

    let mut score = 0u32;

    // Factor 1: entropy (high entropy = suspicious)
    let entropy = compute_entropy(data);
    if entropy > 900 {
        score += 300; // very high entropy — possibly encrypted/packed
    } else if entropy > 700 {
        score += 150;
    } else if entropy > 500 {
        score += 50;
    }

    // Factor 2: null byte ratio (unusual patterns)
    let null_count = data.iter().filter(|&&b| b == 0).count() as u32;
    let null_ratio = (null_count * 1000) / data.len() as u32;
    if null_ratio > 900 {
        score += 50; // mostly zeros — maybe uninitialized
    } else if null_ratio < 50 {
        score += 100; // very few zeros — possibly obfuscated
    }

    // Factor 3: printable ASCII ratio
    let printable = data.iter().filter(|&&b| b >= 0x20 && b <= 0x7E).count() as u32;
    let print_ratio = (printable * 1000) / data.len() as u32;
    if print_ratio > 800 {
        score += 50; // mostly text — could be script/payload
    }

    // Factor 4: repeated patterns (simple RLE detection)
    let mut run_length = 1u32;
    let mut max_run = 1u32;
    for i in 1..data.len() {
        if data[i] == data[i - 1] {
            run_length += 1;
            if run_length > max_run {
                max_run = run_length;
            }
        } else {
            run_length = 1;
        }
    }
    if max_run > data.len() as u32 / 4 {
        score += 100; // long repeated runs — padding/filling
    }

    // Factor 5: suspicious byte sequences (common shellcode patterns)
    let suspicious_patterns: &[(&[u8], u32)] = &[
        (b"\xcd\x80", 150),       // int 0x80 (Linux syscall)
        (b"\x0f\x05", 150),       // syscall instruction
        (b"\xff\xe4", 120),       // jmp esp
        (b"\xff\xe0", 120),       // jmp eax
        (b"\xeb\xfe", 100),       // jmp -2 (infinite loop)
        (b"\x90\x90", 60),        // NOP sled
    ];
    for &(pattern, weight) in suspicious_patterns {
        if match_pattern(data, pattern) > 0 {
            score += weight;
        }
    }

    score.min(1000)
}

// ── Core Scan Operation ─────────────────────────────────────────────────────

/// Execute a scan of a process data buffer against the signature database.
/// Returns a ScanResult with match information.
pub fn execute_scan(
    pid: u32,
    data: &[u8],
    scan_type: u8,
    heuristic_level: u8,
    tick: u64,
) -> ScanResult {
    let mut result = ScanResult {
        scan_id:      0,
        pid:          pid,
        matched:      false,
        match_count:  0,
        max_severity: ThreatLevel::Low,
        composite:    0,
        timestamp:    tick,
        sig_ids:      [0u32; 8],
        sig_count:    0,
        status:       1, // complete
    };

    let mut total_score = 0u32;
    let mut sig_fill = 0usize;

    // Phase 1: Signature matching
    {
        let db = SIG_DB.lock();
        for i in 0..MAX_SIGNATURES {
            let sig = &db.entries[i];
            if sig.id == 0 || !sig.enabled {
                continue;
            }
            if sig.sig_type != 0 {
                continue; // skip non-pattern sigs in this phase
            }
            let pat_len = sig.pattern_len as usize;
            if pat_len == 0 || pat_len > 16 {
                continue;
            }
            let matches = match_pattern(data, &sig.pattern[..pat_len]);
            if matches > 0 {
                result.matched = true;
                result.match_count += matches;
                total_score += sig.base_score;
                if sig.severity > result.max_severity {
                    result.max_severity = sig.severity;
                }
                if sig_fill < 8 {
                    result.sig_ids[sig_fill] = sig.id;
                    sig_fill += 1;
                }
            }
        }
    }

    // Phase 2: Heuristic analysis (based on heuristic_level)
    if heuristic_level > 0 {
        let h_score = heuristic_analyze(data);
        if h_score > 0 {
            // Scale heuristic score based on aggressiveness level
            let scaled = match heuristic_level {
                1 => h_score / 4,
                2 => h_score / 2,
                3 => h_score,
                _ => h_score / 4,
            };
            total_score = total_score.saturating_add(scaled);
            if scaled > 300 {
                result.matched = true;
                if score_to_level(scaled) > result.max_severity {
                    result.max_severity = score_to_level(scaled);
                }
            }
        }
    }

    // Phase 3: Hash-based heuristic matching
    if heuristic_level >= 2 {
        let hash = fnv1a_hash(data);
        let db = SIG_DB.lock();
        for i in 0..MAX_SIGNATURES {
            let sig = &db.entries[i];
            if sig.id == 0 || !sig.enabled || sig.sig_type != 1 {
                continue;
            }
            // For heuristic sigs, check if the hash falls in a suspicious range
            let pat_len = sig.pattern_len as usize;
            if pat_len >= 4 {
                let sig_hash = u32::from_le_bytes([
                    sig.pattern[0], sig.pattern[1],
                    sig.pattern[2], sig.pattern[3],
                ]);
                // Check proximity within a tolerance window
                let diff = if hash > sig_hash { hash - sig_hash } else { sig_hash - hash };
                if diff < 0x0100 {
                    result.matched = true;
                    total_score = total_score.saturating_add(sig.base_score / 2);
                    if sig.severity > result.max_severity {
                        result.max_severity = sig.severity;
                    }
                    if sig_fill < 8 {
                        result.sig_ids[sig_fill] = sig.id;
                        sig_fill += 1;
                    }
                }
            }
        }
    }

    result.composite = total_score.min(1000);
    result.sig_count = sig_fill as u8;

    // If threat detected, record it
    if result.matched && result.composite >= 250 {
        let mut rec = ThreatRecord::empty();
        rec.pid = pid;
        rec.level = score_to_level(result.composite);
        rec.category = ThreatCategory::Malware;
        rec.score = result.composite;
        rec.timestamp = tick;
        rec.contained = false;
        rec.resolved = false;
        // Copy first matched sig name if available
        if result.sig_count > 0 {
            let db = SIG_DB.lock();
            if let Some(sig) = db.find_by_id(result.sig_ids[0]) {
                rec.category = sig.category;
                let copy_len = sig.name_len as usize;
                if copy_len > 0 {
                    let cl = copy_len.min(MAX_SIG_NAME);
                    rec.sig_name[..cl].copy_from_slice(&sig.name[..cl]);
                    rec.sig_len = cl as u8;
                }
            }
        }
        let _ = record_threat(&rec);
        stat_threats_inc();
        if rec.level == ThreatLevel::Critical {
            stat_critical_inc();
        }
    }

    result
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Add a signature to the database.
pub fn add_signature(entry: &SignatureEntry) -> Option<u32> {
    let mut db = SIG_DB.lock();
    db.insert(entry)
}

/// Disable a signature by ID.
pub fn disable_signature(id: u32) -> bool {
    let mut db = SIG_DB.lock();
    db.disable(id)
}

/// Enable a signature by ID.
pub fn enable_signature(id: u32) -> bool {
    let mut db = SIG_DB.lock();
    db.enable(id)
}

/// Enqueue a scan request.
pub fn queue_scan(pid: u32, scan_type: u8, priority: u8, tick: u64) -> Option<u32> {
    let mut queue = SCAN_QUEUE.lock();
    queue.enqueue(pid, scan_type, priority, tick)
}

/// Dequeue the next scan request (highest priority first).
pub fn next_scan_request() -> Option<ScanRequest> {
    let mut queue = SCAN_QUEUE.lock();
    queue.dequeue_next()
}

/// Mark a scan request as completed.
pub fn complete_scan_request(id: u32, tick: u64) -> bool {
    let mut queue = SCAN_QUEUE.lock();
    queue.complete(id, tick)
}

/// Store a scan result.
pub fn store_scan_result(result: &ScanResult) -> Option<u32> {
    let mut store = SCAN_RESULTS.lock();
    store.store(result)
}

/// Retrieve a scan result by ID.
pub fn get_scan_result(id: u32) -> Option<ScanResult> {
    let store = SCAN_RESULTS.lock();
    store.get(id)
}

/// Retrieve the most recent scan result for a PID.
pub fn get_latest_scan_result(pid: u32) -> Option<ScanResult> {
    let store = SCAN_RESULTS.lock();
    store.get_by_pid(pid)
}

/// Set a scan profile at a given index.
pub fn set_scan_profile(idx: usize, profile: &ScanProfile) -> bool {
    let mut store = SCAN_PROFILES.lock();
    store.set(idx, profile)
}

/// Get a scan profile by index.
pub fn get_scan_profile(idx: usize) -> Option<ScanProfile> {
    let store = SCAN_PROFILES.lock();
    store.get(idx)
}

/// Get the currently active scan profile.
pub fn active_scan_profile() -> Option<ScanProfile> {
    let store = SCAN_PROFILES.lock();
    store.active_profile()
}

/// Get the number of pending scans in the queue.
pub fn pending_scan_count() -> u32 {
    let queue = SCAN_QUEUE.lock();
    queue.pending_count()
}

// ── Periodic Scan Scheduler ─────────────────────────────────────────────────

/// Maximum processes tracked for periodic scanning.
const PERIODIC_MAX_PROCS: usize = 32;

/// Periodic scan state for a process.
#[repr(C)]
#[derive(Clone, Copy)]
struct PeriodicEntry {
    pid:          u32,
    interval:     u64,   // ticks between scans
    last_scan:    u64,   // tick of last scan
    scan_type:    u8,
    priority:     u8,
    active:       bool,
}

impl PeriodicEntry {
    const fn empty() -> Self {
        PeriodicEntry {
            pid:       0,
            interval:  DEFAULT_SCAN_INTERVAL_TICKS,
            last_scan: 0,
            scan_type: 1, // quick scan
            priority:  1,
            active:    false,
        }
    }
}

static PERIODIC_TABLE: Mutex<PeriodicTable> = Mutex::new(PeriodicTable::new());

struct PeriodicTable {
    entries: [PeriodicEntry; PERIODIC_MAX_PROCS],
}

impl PeriodicTable {
    const fn new() -> Self {
        PeriodicTable {
            entries: [PeriodicEntry::empty(); PERIODIC_MAX_PROCS],
        }
    }

    fn register(&mut self, pid: u32, interval: u64, scan_type: u8, priority: u8) -> bool {
        // Update if already exists
        for i in 0..PERIODIC_MAX_PROCS {
            if self.entries[i].pid == pid {
                self.entries[i].interval = interval;
                self.entries[i].scan_type = scan_type;
                self.entries[i].priority = priority;
                self.entries[i].active = true;
                return true;
            }
        }
        // Find empty slot
        for i in 0..PERIODIC_MAX_PROCS {
            if self.entries[i].pid == 0 || !self.entries[i].active {
                self.entries[i] = PeriodicEntry {
                    pid:       pid,
                    interval:  interval,
                    last_scan: 0,
                    scan_type: scan_type,
                    priority:  priority,
                    active:    true,
                };
                return true;
            }
        }
        false
    }

    fn unregister(&mut self, pid: u32) -> bool {
        for i in 0..PERIODIC_MAX_PROCS {
            if self.entries[i].pid == pid {
                self.entries[i].active = false;
                self.entries[i].pid = 0;
                return true;
            }
        }
        false
    }

    /// Check which processes are due for a scan and enqueue them.
    fn tick(&mut self, current_tick: u64) -> u32 {
        let mut enqueued = 0u32;
        for i in 0..PERIODIC_MAX_PROCS {
            if !self.entries[i].active || self.entries[i].pid == 0 {
                continue;
            }
            let elapsed = current_tick.saturating_sub(self.entries[i].last_scan);
            if elapsed >= self.entries[i].interval {
                if queue_scan(
                    self.entries[i].pid,
                    self.entries[i].scan_type,
                    self.entries[i].priority,
                    current_tick,
                ).is_some() {
                    self.entries[i].last_scan = current_tick;
                    enqueued += 1;
                }
            }
        }
        enqueued
    }
}

/// Register a process for periodic scanning.
pub fn register_periodic_scan(pid: u32, interval_ticks: u64, scan_type: u8, priority: u8) -> bool {
    let mut table = PERIODIC_TABLE.lock();
    table.register(pid, interval_ticks, scan_type, priority)
}

/// Unregister a process from periodic scanning.
pub fn unregister_periodic_scan(pid: u32) -> bool {
    let mut table = PERIODIC_TABLE.lock();
    table.unregister(pid)
}

/// Called periodically to enqueue scans that are due.
/// Returns the number of scans enqueued.
pub fn periodic_scan_tick(current_tick: u64) -> u32 {
    let mut table = PERIODIC_TABLE.lock();
    table.tick(current_tick)
}

// ── Scanner Statistics ──────────────────────────────────────────────────────

static STATS_SCANS_TOTAL:     AtomicU64 = AtomicU64::new(0);
static STATS_SCANS_MATCHED:   AtomicU64 = AtomicU64::new(0);
static STATS_SCANS_QUEUED:    AtomicU64 = AtomicU64::new(0);
static STATS_SIG_DB_SIZE:     AtomicU32 = AtomicU32::new(0);
static STATS_SCANNER_INIT:    AtomicBool = AtomicBool::new(false);

/// Initialize the scanner module.
pub fn scanner_init() {
    // Load default signatures
    {
        let mut db = SIG_DB.lock();
        let default_sigs: &[(&[u8], u8, ThreatCategory, ThreatLevel, u32, &[u8])] = &[
            // (pattern, sig_type, category, severity, base_score, name)
            (b"\xcd\x80\x00\x00", 0, ThreatCategory::Intrusion,    ThreatLevel::High,     700, b"linux_syscall_int80"),
            (b"\x0f\x05\x00\x00", 0, ThreatCategory::Intrusion,    ThreatLevel::High,     700, b"linux_syscall_new"),
            (b"\xff\xe4\x00\x00", 0, ThreatCategory::Intrusion,    ThreatLevel::Critical, 850, b"jmp_esp_shellcode"),
            (b"\xff\xe0\x00\x00", 0, ThreatCategory::Intrusion,    ThreatLevel::Critical, 850, b"jmp_eax_shellcode"),
            (b"\x90\x90\x90\x90", 0, ThreatCategory::Malware,      ThreatLevel::Medium,   500, b"nop_sled_long"),
            (b"\xeb\xfe\x90\x90", 0, ThreatCategory::Malware,      ThreatLevel::Medium,   450, b"infinite_loop_nop"),
            (b"\xcc\xcc\xcc\xcc", 0, ThreatCategory::Anomaly,      ThreatLevel::Low,      200, b"int3_breakpoint"),
            (b"\x48\x31\xc0\x48", 0, ThreatCategory::Intrusion,    ThreatLevel::High,     600, b"xor_rax_x64_seq"),
        ];

        for &(pattern, sig_type, category, severity, base_score, name) in default_sigs {
            let mut entry = SignatureEntry::empty();
            entry.sig_type = sig_type;
            entry.category = category;
            entry.severity = severity;
            entry.base_score = base_score;
            let plen = pattern.len().min(16);
            entry.pattern[..plen].copy_from_slice(&pattern[..plen]);
            entry.pattern_len = plen as u8;
            let nlen = name.len().min(MAX_SIG_NAME);
            entry.name[..nlen].copy_from_slice(&name[..nlen]);
            entry.name_len = nlen as u8;
            entry.enabled = true;
            let _ = db.insert(&entry);
        }
    }

    // Set default scan profile
    {
        let mut store = SCAN_PROFILES.lock();
        let default_profile = ScanProfile {
            name:            [
                b's', b't', b'a', b'n', b'd', b'a', b'r', b'd',
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            name_len:        8,
            scan_memory:     true,
            scan_syscalls:   true,
            scan_network:    true,
            scan_fs:         false,
            scan_behavior:   true,
            max_depth:       3,
            timeout_ticks:   500,
            heuristic_level: 2,
            enabled:         true,
        };
        let _ = store.set(0, &default_profile);
    }

    STATS_SIG_DB_SIZE.store(8, Ordering::Release);
    STATS_SCANNER_INIT.store(true, Ordering::Release);
}

/// Check if the scanner has been initialized.
pub fn scanner_is_init() -> bool {
    STATS_SCANNER_INIT.load(Ordering::Acquire)
}

/// Record that a scan was executed.
pub fn stat_scan_executed(matched: bool) {
    STATS_SCANS_TOTAL.fetch_add(1, Ordering::Relaxed);
    if matched {
        STATS_SCANS_MATCHED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Record that a scan was queued.
pub fn stat_scan_queued() {
    STATS_SCANS_QUEUED.fetch_add(1, Ordering::Relaxed);
}

/// Get scanner statistics.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScannerStats {
    pub scans_total:   u64,
    pub scans_matched: u64,
    pub scans_queued:  u64,
    pub sig_db_size:   u32,
    pub pending_count: u32,
}

/// Retrieve current scanner statistics.
pub fn get_scanner_stats() -> ScannerStats {
    ScannerStats {
        scans_total:   STATS_SCANS_TOTAL.load(Ordering::Relaxed),
        scans_matched: STATS_SCANS_MATCHED.load(Ordering::Relaxed),
        scans_queued:  STATS_SCANS_QUEUED.load(Ordering::Relaxed),
        sig_db_size:   STATS_SIG_DB_SIZE.load(Ordering::Relaxed),
        pending_count: pending_scan_count(),
    }
}
