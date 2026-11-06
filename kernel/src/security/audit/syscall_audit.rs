// kernel/src/security/audit/syscall_audit.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Syscall Audit — Integration of audit log into syscall dispatch
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • Called by SYSCALL/SYSENTER handler, before and after execution
//   • `audit_syscall_entry`: decides if call must be logged/blocked
//   • `audit_syscall_exit`: completes record with result
//   • Correlates entry+exit via per-thread context (SyscallContext)
//
// RULE SAU-01: audit_syscall_entry/exit must not trigger recursive syscall
//              (no FS access, no network).
// RULE SAU-02: If audit returns AuditVerdict::Kill, thread is killed IMMEDIATELY.
// RULE SAU-03: Kernel private syscalls (nr > MAX_USER_SYSCALL) not audited here.
// ═══════════════════════════════════════════════════════════════════════════════

use super::logger::{log_event, log_security_violation, AuditCategory, AuditOutcome};
use super::rules::{evaluate_global, RuleAction};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Audit verdict
// ─────────────────────────────────────────────────────────────────────────────

/// Verdict returned by `audit_syscall_entry` to syscall dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditVerdict {
    /// Allow execution of syscall.
    Allow,
    /// Block syscall with EPERM.
    DenyEperm,
    /// Block syscall with ENOSYS.
    DenyEnosys,
    /// Kill calling thread.
    Kill,
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-thread syscall context (to correlate entry/exit)
// ─────────────────────────────────────────────────────────────────────────────

/// Context of an in-flight syscall.
#[derive(Clone, Copy)]
struct SyscallContext {
    thread_id: u32,
    pid: u32,
    uid: u16,
    syscall_nr: u32,
    #[allow(dead_code)]
    entry_tsc: u64,
    #[allow(dead_code)]
    verdict: AuditVerdict,
    active: bool,
}

impl SyscallContext {
    const fn empty() -> Self {
        Self {
            thread_id: 0,
            pid: 0,
            uid: 0,
            syscall_nr: 0,
            entry_tsc: 0,
            verdict: AuditVerdict::Allow,
            active: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-thread context table
// ─────────────────────────────────────────────────────────────────────────────

const MAX_CONCURRENT_SYSCALLS: usize = 1024;

struct ContextTable {
    entries: [SyscallContext; MAX_CONCURRENT_SYSCALLS],
}

impl ContextTable {
    const fn new() -> Self {
        Self {
            entries: [SyscallContext::empty(); MAX_CONCURRENT_SYSCALLS],
        }
    }

    fn slot(tid: u32) -> usize {
        (tid as usize) % MAX_CONCURRENT_SYSCALLS
    }

    fn set(&mut self, ctx: SyscallContext) {
        self.entries[Self::slot(ctx.thread_id)] = ctx;
    }

    fn get(&self, tid: u32) -> Option<&SyscallContext> {
        let e = &self.entries[Self::slot(tid)];
        if e.active && e.thread_id == tid {
            Some(e)
        } else {
            None
        }
    }

    fn clear(&mut self, tid: u32) {
        let slot = Self::slot(tid);
        if self.entries[slot].thread_id == tid {
            self.entries[slot].active = false;
        }
    }
}

static CTX_TABLE: spin::Mutex<ContextTable> = spin::Mutex::new(ContextTable::new());

// ─────────────────────────────────────────────────────────────────────────────
// User syscall bound
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum syscall number accessible to userspace.
const MAX_USER_SYSCALL: u32 = 511;

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

static ENTRIES_TOTAL: AtomicU64 = AtomicU64::new(0);
static EXITS_TOTAL: AtomicU64 = AtomicU64::new(0);
static DENIALS_TOTAL: AtomicU64 = AtomicU64::new(0);
static KILLS_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORPHAN_EXITS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Timestamp
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdtsc available on x86_64; non-serialized sufficient for audit timestamp.
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    }
    (hi as u64) << 32 | lo as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Called BEFORE syscall execution.
///
/// Returns verdict that must be used by the dispatcher.
///
/// # Arguments
/// - `syscall_nr`: syscall number
/// - `pid`: caller PID
/// - `tid`: caller Thread ID
/// - `uid`: caller effective UID
///
/// RULE SAU-03: internal kernel syscalls not audited.
pub fn audit_syscall_entry(syscall_nr: u32, pid: u32, tid: u32, uid: u16) -> AuditVerdict {
    // RULE SAU-03
    if syscall_nr > MAX_USER_SYSCALL {
        return AuditVerdict::Allow;
    }

    ENTRIES_TOTAL.fetch_add(1, Ordering::Relaxed);

    // Evaluate global rules
    let action = evaluate_global(pid, uid as u32, syscall_nr, AuditCategory::Syscall, 0);

    let verdict = match action {
        RuleAction::Skip => AuditVerdict::Allow,
        RuleAction::Log => AuditVerdict::Allow,
        RuleAction::Alert => {
            // Alert and allow
            log_security_violation(
                pid,
                tid,
                uid,
                syscall_nr,
                encode_syscall_data(syscall_nr, "alert"),
            );
            AuditVerdict::Allow
        }
        RuleAction::Deny => {
            DENIALS_TOTAL.fetch_add(1, Ordering::Relaxed);
            log_event(
                AuditCategory::SecurityViolation,
                pid,
                tid,
                uid,
                syscall_nr,
                -1,
                AuditOutcome::Deny,
                encode_syscall_data(syscall_nr, "deny-rule"),
            );
            AuditVerdict::DenyEperm
        }
        RuleAction::Kill => {
            KILLS_TOTAL.fetch_add(1, Ordering::Relaxed);
            log_event(
                AuditCategory::SecurityViolation,
                pid,
                tid,
                uid,
                syscall_nr,
                -1,
                AuditOutcome::Kill,
                encode_syscall_data(syscall_nr, "kill-rule"),
            );
            AuditVerdict::Kill
        }
    };

    // Save context for exit
    CTX_TABLE.lock().set(SyscallContext {
        thread_id: tid,
        pid,
        uid,
        syscall_nr,
        entry_tsc: rdtsc(),
        verdict,
        active: true,
    });

    // Log entry if Log or Alert
    if matches!(action, RuleAction::Log | RuleAction::Alert) {
        log_event(
            AuditCategory::Syscall,
            pid,
            tid,
            uid,
            syscall_nr,
            0,
            AuditOutcome::Allow,
            encode_syscall_data(syscall_nr, "entry"),
        );
    }

    verdict
}

/// Called AFTER syscall execution, with its result.
///
/// RULE SAU-01: No recursive syscall in this function.
pub fn audit_syscall_exit(tid: u32, result: i64) {
    EXITS_TOTAL.fetch_add(1, Ordering::Relaxed);

    let ctx_data = {
        let mut table = CTX_TABLE.lock();
        let data = table.get(tid).copied();
        table.clear(tid);
        data
    };

    let ctx = match ctx_data {
        Some(c) => c,
        None => {
            ORPHAN_EXITS.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    let outcome = if result < 0 {
        AuditOutcome::Error
    } else {
        AuditOutcome::Allow
    };

    // Record exit with result
    log_event(
        AuditCategory::Syscall,
        ctx.pid,
        ctx.thread_id,
        ctx.uid,
        ctx.syscall_nr,
        result as i32,
        outcome,
        encode_syscall_data(ctx.syscall_nr, "exit"),
    );
}

/// Logs a capability denial (called by capability::verify).
pub fn audit_capability_deny(pid: u32, tid: u32, uid: u16, cap_right: u32) {
    let mut data = [0u8; 8];
    data[..4].copy_from_slice(&cap_right.to_le_bytes());
    log_event(
        AuditCategory::Capability,
        pid,
        tid,
        uid,
        0,
        -1,
        AuditOutcome::Deny,
        data,
    );
}

/// Logs a denied file access.
pub fn audit_file_deny(pid: u32, tid: u32, uid: u16, path_hash: u32) {
    let mut data = [0u8; 8];
    data[..4].copy_from_slice(&path_hash.to_le_bytes());
    log_event(
        AuditCategory::FileAccess,
        pid,
        tid,
        uid,
        0,
        -1,
        AuditOutcome::Deny,
        data,
    );
}

/// Encodes context data into 8 bytes.
fn encode_syscall_data(syscall_nr: u32, _tag: &str) -> [u8; 8] {
    let mut d = [0u8; 8];
    d[..4].copy_from_slice(&syscall_nr.to_le_bytes());
    d
}

#[derive(Debug, Clone, Copy)]
pub struct SyscallAuditStats {
    pub entries_total: u64,
    pub exits_total: u64,
    pub denials_total: u64,
    pub kills_total: u64,
    pub orphan_exits: u64,
}

pub fn syscall_audit_stats() -> SyscallAuditStats {
    SyscallAuditStats {
        entries_total: ENTRIES_TOTAL.load(Ordering::Relaxed),
        exits_total: EXITS_TOTAL.load(Ordering::Relaxed),
        denials_total: DENIALS_TOTAL.load(Ordering::Relaxed),
        kills_total: KILLS_TOTAL.load(Ordering::Relaxed),
        orphan_exits: ORPHAN_EXITS.load(Ordering::Relaxed),
    }
}
