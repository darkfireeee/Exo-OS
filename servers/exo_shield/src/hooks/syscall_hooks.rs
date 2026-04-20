//! # syscall_hooks — Syscall interception & anomaly detection
//!
//! Monitors system call activity to detect:
//! - **Frequency anomalies**: PIDs issuing syscalls at an abnormal rate
//! - **Dangerous syscalls**: ptrace, execve, mount, and other
//!   privilege-escalation vectors
//! - **Sequence patterns**: known attack sequences (fork→ptrace→execve,
//!   clone→mount→chroot, etc.)
//!
//! All data structures are static arrays. No heap, no `Vec`, no `String`.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum per-PID syscall frequency tracking entries.
const MAX_FREQ_ENTRIES: usize = 128;

/// Maximum syscall sequence entries (recent sequences per PID).
const MAX_SEQ_ENTRIES: usize = 256;

/// Maximum recent syscall events stored.
const MAX_SYSCALL_EVENTS: usize = 1024;

/// Maximum number of dangerous syscall numbers tracked.
const MAX_DANGEROUS_SYSCALLS: usize = 16;

/// Maximum sequence length for pattern analysis.
const MAX_SEQ_LENGTH: usize = 8;

/// Maximum number of known attack sequences.
const MAX_KNOWN_SEQUENCES: usize = 16;

/// Syscall frequency threshold per PID within window.
const SYSCALL_RATE_THRESHOLD: u32 = 1024;

/// Tracking window in TSC ticks (~1 second at 3 GHz).
const TRACKING_WINDOW_TSC: u64 = 3_000_000_000;

// ── Linux x86_64 syscall numbers ──────────────────────────────────────────────

/// ptrace (attach to another process).
pub const SYS_PTRACE: u32 = 101;
/// execve (execute a program).
pub const SYS_EXECVE: u32 = 59;
/// mount (mount a filesystem).
pub const SYS_MOUNT: u32 = 165;
/// clone (create a child process).
pub const SYS_CLONE: u32 = 56;
/// fork (create a child process, legacy).
pub const SYS_FORK: u32 = 57;
/// chroot (change root directory).
pub const SYS_CHROOT: u32 = 161;
/// setuid (set user identity).
pub const SYS_SETUID: u32 = 105;
/// setgid (set group identity).
pub const SYS_SETGID: u32 = 106;
/// prctl (process control — can disable ptrace).
pub const SYS_PRCTL: u32 = 157;
/// reboot (reboot the system).
pub const SYS_REBOOT: u32 = 169;
/// kexec_load (load a new kernel).
pub const SYS_KEXEC_LOAD: u32 = 246;
/// init_module (load a kernel module).
pub const SYS_INIT_MODULE: u32 = 175;
/// delete_module (unload a kernel module).
pub const SYS_DELETE_MODULE: u32 = 176;
/// bpf (load BPF program — potential escalation).
pub const SYS_BPF: u32 = 321;
/// keyctl (key management — potential info leak).
pub const SYS_KEYCTL: u32 = 250;
/// perf_event_open (performance monitoring — can leak data).
pub const SYS_PERF_EVENT_OPEN: u32 = 298;

// ── TSC read ──────────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Syscall event recorded for each monitored system call.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SyscallEvent {
    /// PID that issued the syscall.
    pub pid: u32,
    /// Syscall number.
    pub syscall_nr: u32,
    /// First 3 arguments (compressed into 24 bytes).
    pub args: [u64; 3],
    /// Return value (0 = success, negative = error).
    pub ret_val: i64,
    /// Flags: bit 0 = blocked, bit 1 = dangerous, bit 2 = sequence_match.
    pub flags: u8,
    /// Padding.
    pub _pad: [u8; 7],
    /// TSC timestamp.
    pub timestamp: u64,
}

impl Default for SyscallEvent {
    fn default() -> Self {
        Self {
            pid: 0,
            syscall_nr: 0,
            args: [0; 3],
            ret_val: 0,
            flags: 0,
            _pad: [0; 7],
            timestamp: 0,
        }
    }
}

/// Per-PID syscall frequency tracking entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SyscallFreqEntry {
    /// PID being tracked.
    pub pid: u32,
    /// Number of syscalls in the current window.
    pub count: u32,
    /// TSC of the window start.
    pub window_start: u64,
    /// Whether this PID has been flagged for rate anomaly.
    pub flagged: u8,
    /// Padding.
    pub _pad: [u8; 3],
}

impl Default for SyscallFreqEntry {
    fn default() -> Self {
        Self {
            pid: 0,
            count: 0,
            window_start: 0,
            flagged: 0,
            _pad: [0; 3],
        }
    }
}

/// Syscall sequence tracking entry (per PID).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SyscallSeqEntry {
    /// PID being tracked.
    pub pid: u32,
    /// Recent syscall numbers in order.
    pub sequence: [u32; MAX_SEQ_LENGTH],
    /// Current position in the sequence ring.
    pub seq_pos: u32,
    /// Length of the recorded sequence.
    pub seq_len: u32,
    /// Whether a known attack pattern was matched.
    pub matched_pattern: u8,
    /// Padding.
    pub _pad: [u8; 3],
}

impl Default for SyscallSeqEntry {
    fn default() -> Self {
        Self {
            pid: 0,
            sequence: [0; MAX_SEQ_LENGTH],
            seq_pos: 0,
            seq_len: 0,
            matched_pattern: 0,
            _pad: [0; 3],
        }
    }
}

/// Known attack sequence pattern.
struct AttackPattern {
    /// Sequence of syscall numbers.
    sequence: [u32; MAX_SEQ_LENGTH],
    /// Length of the pattern.
    len: u8,
    /// Threat level: 0 = low, 1 = medium, 2 = high, 3 = critical.
    threat_level: u8,
    /// Active flag.
    active: AtomicU8,
}

impl AttackPattern {
    const fn new(seq: [u32; MAX_SEQ_LENGTH], len: u8, threat: u8) -> Self {
        Self {
            sequence: seq,
            len,
            threat_level: threat,
            active: AtomicU8::new(1),
        }
    }
}

// ── Static storage ────────────────────────────────────────────────────────────

/// Ring buffer of recent syscall events.
static SYSCALL_EVENTS: Mutex<[SyscallEvent; MAX_SYSCALL_EVENTS]> = Mutex::new(
    [SyscallEvent::default(); MAX_SYSCALL_EVENTS],
);
static SYSCALL_EVENT_IDX: AtomicU32 = AtomicU32::new(0);

/// Per-PID frequency tracking table.
static FREQ_TABLE: Mutex<[SyscallFreqEntry; MAX_FREQ_ENTRIES]> = Mutex::new(
    [SyscallFreqEntry::default(); MAX_FREQ_ENTRIES],
);

/// Per-PID sequence tracking table.
static SEQ_TABLE: Mutex<[SyscallSeqEntry; MAX_SEQ_ENTRIES]> = Mutex::new(
    [SyscallSeqEntry::default(); MAX_SEQ_ENTRIES],
);

/// Dangerous syscall numbers table.
static DANGEROUS_SYSCALLS: Mutex<[u32; MAX_DANGEROUS_SYSCALLS]> = Mutex::new(
    [SYS_PTRACE, SYS_EXECVE, SYS_MOUNT, SYS_CHROOT,
     SYS_SETUID, SYS_SETGID, SYS_PRCTL, SYS_REBOOT,
     SYS_KEXEC_LOAD, SYS_INIT_MODULE, SYS_DELETE_MODULE,
     SYS_BPF, SYS_KEYCTL, SYS_PERF_EVENT_OPEN,
     SYS_CLONE, SYS_FORK],
);

/// Known attack patterns:
///   0: fork→ptrace→execve (classic debugger attach exploit)
///   1: clone→mount→chroot (container breakout)
///   2: ptrace→setuid→execve (privilege escalation via ptrace)
///   3: fork→fork→execve (fork bomb / shellcode)
///   4: mount→chroot→setuid (full root escape)
///   5: prctl→ptrace→execve (ptrace scope bypass)
///   6: clone→ptrace→setuid (container + privilege attack)
///   7: kexec_load→reboot (kernel replacement attack)
static ATTACK_PATTERNS: [AttackPattern; MAX_KNOWN_SEQUENCES] = [
    // 0: fork→ptrace→execve
    AttackPattern::new([SYS_FORK, SYS_PTRACE, SYS_EXECVE, 0, 0, 0, 0, 0], 3, 3),
    // 1: clone→mount→chroot
    AttackPattern::new([SYS_CLONE, SYS_MOUNT, SYS_CHROOT, 0, 0, 0, 0, 0], 3, 2),
    // 2: ptrace→setuid→execve
    AttackPattern::new([SYS_PTRACE, SYS_SETUID, SYS_EXECVE, 0, 0, 0, 0, 0], 3, 3),
    // 3: fork→fork→execve (fork bomb pattern)
    AttackPattern::new([SYS_FORK, SYS_FORK, SYS_EXECVE, 0, 0, 0, 0, 0], 3, 1),
    // 4: mount→chroot→setuid
    AttackPattern::new([SYS_MOUNT, SYS_CHROOT, SYS_SETUID, 0, 0, 0, 0, 0], 3, 3),
    // 5: prctl→ptrace→execve
    AttackPattern::new([SYS_PRCTL, SYS_PTRACE, SYS_EXECVE, 0, 0, 0, 0, 0], 3, 3),
    // 6: clone→ptrace→setuid
    AttackPattern::new([SYS_CLONE, SYS_PTRACE, SYS_SETUID, 0, 0, 0, 0, 0], 3, 3),
    // 7: kexec_load→reboot
    AttackPattern::new([SYS_KEXEC_LOAD, SYS_REBOOT, 0, 0, 0, 0, 0, 0], 2, 3),
    // 8: init_module→delete_module (module insertion/removal)
    AttackPattern::new([SYS_INIT_MODULE, SYS_DELETE_MODULE, 0, 0, 0, 0, 0, 0], 2, 2),
    // 9: bpf→setuid (BPF escalation)
    AttackPattern::new([SYS_BPF, SYS_SETUID, 0, 0, 0, 0, 0, 0], 2, 2),
    // 10: perf_event_open→setuid (perf escalation)
    AttackPattern::new([SYS_PERF_EVENT_OPEN, SYS_SETUID, 0, 0, 0, 0, 0, 0], 2, 2),
    // 11: keyctl→setuid (keyring escalation)
    AttackPattern::new([SYS_KEYCTL, SYS_SETUID, 0, 0, 0, 0, 0, 0], 2, 2),
    // 12-15: reserved (inactive)
    AttackPattern::new([0; MAX_SEQ_LENGTH], 0, 0),
    AttackPattern::new([0; MAX_SEQ_LENGTH], 0, 0),
    AttackPattern::new([0; MAX_SEQ_LENGTH], 0, 0),
    AttackPattern::new([0; MAX_SEQ_LENGTH], 0, 0),
];

/// Statistics counters.
static TOTAL_SYSCALLS: AtomicU64 = AtomicU64::new(0);
static DANGEROUS_SYSCALL_COUNT: AtomicU64 = AtomicU64::new(0);
static BLOCKED_SYSCALLS: AtomicU64 = AtomicU64::new(0);
static RATE_ANOMALIES: AtomicU64 = AtomicU64::new(0);
static SEQUENCE_MATCHES: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Store a syscall event into the ring buffer.
fn store_syscall_event(event: SyscallEvent) {
    let mut events = SYSCALL_EVENTS.lock();
    let idx = SYSCALL_EVENT_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_SYSCALL_EVENTS;
    events[idx] = event;
}

/// Check if a syscall number is in the dangerous list.
fn is_dangerous_syscall(syscall_nr: u32) -> bool {
    let table = DANGEROUS_SYSCALLS.lock();
    for i in 0..MAX_DANGEROUS_SYSCALLS {
        if table[i] == syscall_nr {
            return true;
        }
    }
    false
}

/// Update the frequency table for a PID.
/// Returns `true` if rate anomaly is detected.
fn update_freq(pid: u32) -> bool {
    let mut table = FREQ_TABLE.lock();
    let now = read_tsc();

    // Find existing entry
    for i in 0..MAX_FREQ_ENTRIES {
        if table[i].pid == pid {
            let elapsed = now.wrapping_sub(table[i].window_start);
            if elapsed > TRACKING_WINDOW_TSC {
                table[i].count = 1;
                table[i].window_start = now;
                table[i].flagged = 0;
                return false;
            }
            table[i].count += 1;
            if table[i].count >= SYSCALL_RATE_THRESHOLD && table[i].flagged == 0 {
                table[i].flagged = 1;
                RATE_ANOMALIES.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            return table[i].flagged != 0;
        }
    }

    // Find empty slot
    for i in 0..MAX_FREQ_ENTRIES {
        if table[i].pid == 0 {
            table[i].pid = pid;
            table[i].count = 1;
            table[i].window_start = now;
            table[i].flagged = 0;
            return false;
        }
    }

    // Evict oldest
    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..MAX_FREQ_ENTRIES {
        if table[i].window_start < oldest_tsc {
            oldest_tsc = table[i].window_start;
            oldest_idx = i;
        }
    }
    table[oldest_idx].pid = pid;
    table[oldest_idx].count = 1;
    table[oldest_idx].window_start = now;
    table[oldest_idx].flagged = 0;
    false
}

/// Append a syscall to the sequence tracker for a PID.
/// Returns the index of the matched attack pattern (0xFF = no match).
fn update_sequence(pid: u32, syscall_nr: u32) -> u8 {
    let mut table = SEQ_TABLE.lock();

    // Find existing entry for this PID
    let entry_idx = {
        let mut found = None;
        for i in 0..MAX_SEQ_ENTRIES {
            if table[i].pid == pid {
                found = Some(i);
                break;
            }
        }
        found
    };

    let idx = match entry_idx {
        Some(i) => i,
        None => {
            // Create new entry
            let mut slot = None;
            for i in 0..MAX_SEQ_ENTRIES {
                if table[i].pid == 0 {
                    slot = Some(i);
                    break;
                }
            }
            let slot = match slot {
                Some(s) => s,
                None => {
                    // Evict oldest (lowest seq_len with smallest timestamp)
                    let mut evict = 0usize;
                    let mut min_len = u32::MAX;
                    for i in 0..MAX_SEQ_ENTRIES {
                        if table[i].seq_len < min_len {
                            min_len = table[i].seq_len;
                            evict = i;
                        }
                    }
                    evict
                }
            };
            table[slot].pid = pid;
            table[slot].sequence = [0; MAX_SEQ_LENGTH];
            table[slot].seq_pos = 0;
            table[slot].seq_len = 0;
            table[slot].matched_pattern = 0;
            slot
        }
    };

    // Append syscall to the sequence ring
    let pos = table[idx].seq_pos as usize % MAX_SEQ_LENGTH;
    table[idx].sequence[pos] = syscall_nr;
    table[idx].seq_pos = table[idx].seq_pos.wrapping_add(1) % MAX_SEQ_LENGTH as u32;
    if table[idx].seq_len < MAX_SEQ_LENGTH as u32 {
        table[idx].seq_len += 1;
    }

    // Check against known attack patterns
    let effective_len = table[idx].seq_len as usize;
    let start_pos = if effective_len < MAX_SEQ_LENGTH {
        0
    } else {
        table[idx].seq_pos as usize
    };

    for pat_idx in 0..MAX_KNOWN_SEQUENCES {
        let pat = &ATTACK_PATTERNS[pat_idx];
        if pat.active.load(Ordering::Acquire) == 0 || pat.len == 0 {
            continue;
        }
        let pat_len = pat.len as usize;

        if effective_len < pat_len {
            continue;
        }

        // Compare the last pat_len syscalls with the pattern
        let mut match_found = true;
        for j in 0..pat_len {
            let seq_idx = (start_pos + effective_len - pat_len + j) % MAX_SEQ_LENGTH;
            if table[idx].sequence[seq_idx] != pat.sequence[j] {
                match_found = false;
                break;
            }
        }

        if match_found {
            table[idx].matched_pattern = pat_idx as u8;
            SEQUENCE_MATCHES.fetch_add(1, Ordering::Relaxed);
            return pat_idx as u8;
        }
    }

    0xFF
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Validates a syscall before it is executed.
///
/// Checks:
/// 1. Whether the syscall is in the dangerous list
/// 2. Whether the PID has exceeded the syscall rate limit
///
/// Returns `true` if the syscall should be blocked.
pub fn pre_syscall_check(pid: u32, syscall_nr: u32, _args: [u64; 3]) -> bool {
    TOTAL_SYSCALLS.fetch_add(1, Ordering::Relaxed);

    // Check dangerous syscall
    if is_dangerous_syscall(syscall_nr) {
        DANGEROUS_SYSCALL_COUNT.fetch_add(1, Ordering::Relaxed);

        // ptrace by non-root is always blocked
        if syscall_nr == SYS_PTRACE {
            // In a real system, we'd check the UID here.
            // For the model, ptrace is monitored but not auto-blocked.
        }

        // kexec_load and init_module are always blocked for non-kernel PIDs
        if syscall_nr == SYS_KEXEC_LOAD || syscall_nr == SYS_INIT_MODULE {
            if pid != 0 {
                BLOCKED_SYSCALLS.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }

        // reboot by non-init is blocked
        if syscall_nr == SYS_REBOOT && pid != 1 {
            BLOCKED_SYSCALLS.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }

    // Check rate limit
    if update_freq(pid) {
        // Rate anomaly — block if dangerous syscall
        if is_dangerous_syscall(syscall_nr) {
            BLOCKED_SYSCALLS.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }

    false
}

/// Monitors a syscall after it has been executed.
///
/// Records the event, checks for dangerous syscalls, and performs
/// sequence analysis.
pub fn post_syscall_monitor(
    pid: u32,
    syscall_nr: u32,
    args: [u64; 3],
    ret_val: i64,
) {
    let now = read_tsc();
    let is_dangerous = is_dangerous_syscall(syscall_nr);

    // Update sequence tracker
    let seq_match = update_sequence(pid, syscall_nr);

    let mut flags = 0u8;
    if is_dangerous {
        flags |= 2; // bit 1 = dangerous
    }
    if seq_match != 0xFF {
        flags |= 4; // bit 2 = sequence_match
    }
    if ret_val < 0 {
        // Failed syscall — potential probing
        flags |= 8; // bit 3 = failed
    }

    let event = SyscallEvent {
        pid,
        syscall_nr,
        args,
        ret_val,
        flags,
        _pad: [0; 7],
        timestamp: now,
    };

    store_syscall_event(event);
}

/// Detects whether a specific syscall number is dangerous.
///
/// Returns `Some(threat_description)` if dangerous, `None` otherwise.
/// The threat description is encoded as: high byte = syscall number,
/// low byte = danger level (0-3).
pub fn detect_dangerous_syscall(syscall_nr: u32) -> Option<u32> {
    if is_dangerous_syscall(syscall_nr) {
        // Determine danger level
        let level = match syscall_nr {
            SYS_PTRACE | SYS_KEXEC_LOAD | SYS_INIT_MODULE | SYS_REBOOT => 3u8,
            SYS_MOUNT | SYS_CHROOT | SYS_SETUID | SYS_SETGID => 2u8,
            SYS_EXECVE | SYS_CLONE | SYS_FORK => 1u8,
            SYS_PRCTL | SYS_DELETE_MODULE | SYS_BPF | SYS_KEYCTL | SYS_PERF_EVENT_OPEN => 1u8,
            _ => 0u8,
        };
        Some((syscall_nr << 8) | level as u32)
    } else {
        None
    }
}

/// Analyzes the syscall sequence for a given PID.
///
/// Returns `(matched_pattern_index, threat_level)` if a known attack
/// sequence is detected, `None` otherwise.
pub fn analyze_syscall_sequence(pid: u32) -> Option<(u8, u8)> {
    let table = SEQ_TABLE.lock();

    for i in 0..MAX_SEQ_ENTRIES {
        if table[i].pid == pid && table[i].matched_pattern != 0 {
            let pat_idx = table[i].matched_pattern as usize;
            if pat_idx < MAX_KNOWN_SEQUENCES {
                let threat = ATTACK_PATTERNS[pat_idx].threat_level;
                return Some((table[i].matched_pattern, threat));
            }
        }
    }
    None
}

/// Gets the current syscall frequency for a PID.
///
/// Returns `(count, is_anomalous)`.
pub fn get_syscall_freq(pid: u32) -> (u32, bool) {
    let table = FREQ_TABLE.lock();
    let now = read_tsc();

    for i in 0..MAX_FREQ_ENTRIES {
        if table[i].pid == pid {
            let elapsed = now.wrapping_sub(table[i].window_start);
            if elapsed > TRACKING_WINDOW_TSC {
                return (0, false);
            }
            return (table[i].count, table[i].flagged != 0);
        }
    }
    (0, false)
}

/// Gets the recent syscall sequence for a PID.
///
/// Fills `out` with the syscall numbers in temporal order and returns
/// the number of entries written.
pub fn get_syscall_sequence(pid: u32, out: &mut [u32]) -> usize {
    let table = SEQ_TABLE.lock();

    for i in 0..MAX_SEQ_ENTRIES {
        if table[i].pid == pid {
            let len = table[i].seq_len as usize;
            let start = if len < MAX_SEQ_LENGTH {
                0
            } else {
                table[i].seq_pos as usize
            };
            let written = len.min(out.len());
            for j in 0..written {
                let seq_idx = (start + j) % MAX_SEQ_LENGTH;
                out[j] = table[i].sequence[seq_idx];
            }
            return written;
        }
    }
    0
}

/// Queries recent syscall events for a specific PID.
///
/// Fills `out` with matching events (most recent first) and returns
/// the number of entries written.
pub fn query_syscall_events_for_pid(pid: u32, out: &mut [SyscallEvent]) -> usize {
    let events = SYSCALL_EVENTS.lock();
    let head = SYSCALL_EVENT_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for offset in 0..MAX_SYSCALL_EVENTS {
        let i = (head + MAX_SYSCALL_EVENTS - 1 - offset) % MAX_SYSCALL_EVENTS;
        if events[i].pid == pid && events[i].timestamp != 0 {
            if written < out.len() {
                out[written] = events[i];
                written += 1;
            } else {
                break;
            }
        }
    }
    written
}

/// Syscall subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SyscallStats {
    pub total_syscalls: u64,
    pub dangerous_syscall_count: u64,
    pub blocked_syscalls: u64,
    pub rate_anomalies: u64,
    pub sequence_matches: u64,
}

/// Collects syscall hook statistics.
pub fn get_syscall_stats() -> SyscallStats {
    SyscallStats {
        total_syscalls: TOTAL_SYSCALLS.load(Ordering::Relaxed),
        dangerous_syscall_count: DANGEROUS_SYSCALL_COUNT.load(Ordering::Relaxed),
        blocked_syscalls: BLOCKED_SYSCALLS.load(Ordering::Relaxed),
        rate_anomalies: RATE_ANOMALIES.load(Ordering::Relaxed),
        sequence_matches: SEQUENCE_MATCHES.load(Ordering::Relaxed),
    }
}

/// Resets the syscall hook subsystem.
pub fn syscall_hooks_init() {
    SYSCALL_EVENT_IDX.store(0, Ordering::Release);
    TOTAL_SYSCALLS.store(0, Ordering::Release);
    DANGEROUS_SYSCALL_COUNT.store(0, Ordering::Release);
    BLOCKED_SYSCALLS.store(0, Ordering::Release);
    RATE_ANOMALIES.store(0, Ordering::Release);
    SEQUENCE_MATCHES.store(0, Ordering::Release);
}
