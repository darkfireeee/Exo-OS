// kernel/src/security/audit/rules.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Audit Rules — Audit filtering and triggering rules
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture:
//   • Set of 64 rules maximum (fixed size, no allocation)
//   • Each rule filters by: pid, uid, syscall_nr, category, outcome
//   • Rules are evaluated in priority order (0 = highest priority)
//   • A rule can trigger: LOG, ALERT, KILL, DENY
//
// RULE ARULE-01: Evaluation order is deterministic (increasing index).
// RULE ARULE-02: No rule can disable SecurityViolation.
// ═══════════════════════════════════════════════════════════════════════════════

use super::logger::AuditCategory;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Rule action
// ─────────────────────────────────────────────────────────────────────────────

/// Action triggered by an audit rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RuleAction {
    /// Do nothing (suppression rule).
    Skip = 0,
    /// Record the event.
    Log = 1,
    /// Record and generate an alert.
    Alert = 2,
    /// Deny operation.
    Deny = 3,
    /// Kill process.
    Kill = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditRule — Individual rule
// ─────────────────────────────────────────────────────────────────────────────

/// Individual audit rule.
///
/// All `Option::None` fields mean "match all" for that criterion.
#[derive(Clone, Copy)]
pub struct AuditRule {
    /// PID to monitor (None = all PIDs).
    pub pid: Option<u32>,
    /// UID to monitor (None = all UIDs).
    pub uid: Option<u32>,
    /// Syscall number (None = all syscalls).
    pub syscall_nr: Option<u32>,
    /// Category (None = all).
    pub category: Option<AuditCategory>,
    /// Applies only to given outcomes (bitmask over AuditOutcome u8).
    /// 0xFF = all.
    pub outcome_mask: u8,
    /// Action to trigger if rule matches.
    pub action: RuleAction,
    /// Is the rule active?
    pub enabled: bool,
    /// Priority (0 = highest).
    pub priority: u8,
    /// Match counter.
    pub match_count: u64,
}

impl AuditRule {
    pub const fn new_log_all() -> Self {
        Self {
            pid: None,
            uid: None,
            syscall_nr: None,
            category: None,
            outcome_mask: 0xFF,
            action: RuleAction::Log,
            enabled: true,
            priority: 128,
            match_count: 0,
        }
    }

    pub const fn new_deny_pid(pid: u32) -> Self {
        Self {
            pid: Some(pid),
            uid: None,
            syscall_nr: None,
            category: None,
            outcome_mask: 0xFF,
            action: RuleAction::Deny,
            enabled: true,
            priority: 10,
            match_count: 0,
        }
    }

    pub const fn new_alert_uid(uid: u32) -> Self {
        Self {
            pid: None,
            uid: Some(uid),
            syscall_nr: None,
            category: None,
            outcome_mask: 0xFF,
            action: RuleAction::Alert,
            enabled: true,
            priority: 20,
            match_count: 0,
        }
    }

    pub const fn new_log_syscall(nr: u32) -> Self {
        Self {
            pid: None,
            uid: None,
            syscall_nr: Some(nr),
            category: None,
            outcome_mask: 0xFF,
            action: RuleAction::Log,
            enabled: true,
            priority: 50,
            match_count: 0,
        }
    }

    /// Tests if this rule applies to described event.
    pub fn matches(
        &self,
        pid: u32,
        uid: u32,
        syscall_nr: u32,
        category: AuditCategory,
        outcome: u8,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        // RULE ARULE-02: SecurityViolation cannot be suppressed
        if category == AuditCategory::SecurityViolation && self.action == RuleAction::Skip {
            return false;
        }
        if let Some(p) = self.pid {
            if p != pid {
                return false;
            }
        }
        if let Some(u) = self.uid {
            if u != uid {
                return false;
            }
        }
        if let Some(s) = self.syscall_nr {
            if s != syscall_nr {
                return false;
            }
        }
        if let Some(c) = self.category {
            if c as u8 != category as u8 {
                return false;
            }
        }
        if self.outcome_mask != 0xFF && (self.outcome_mask & (1u8 << outcome)) == 0 {
            return false;
        }
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RuleSet — Rule collection
// ─────────────────────────────────────────────────────────────────────────────

const MAX_RULES: usize = 64;
const NONE_RULE: Option<AuditRule> = None;

pub struct RuleSet {
    rules: [Option<AuditRule>; MAX_RULES],
    count: usize,
}

impl RuleSet {
    pub const fn new() -> Self {
        Self {
            rules: [NONE_RULE; MAX_RULES],
            count: 0,
        }
    }

    /// Adds a rule to RuleSet.
    ///
    /// Returns rule index or `Err` if set is full.
    pub fn add_rule(&mut self, rule: AuditRule) -> Result<usize, ()> {
        if self.count >= MAX_RULES {
            return Err(());
        }
        // Find first free slot
        for i in 0..MAX_RULES {
            if self.rules[i].is_none() {
                self.rules[i] = Some(rule);
                self.count += 1;
                return Ok(i);
            }
        }
        Err(())
    }

    /// Removes rule at index `idx`.
    pub fn remove_rule(&mut self, idx: usize) -> bool {
        if idx >= MAX_RULES || self.rules[idx].is_none() {
            return false;
        }
        self.rules[idx] = None;
        if self.count > 0 {
            self.count -= 1;
        }
        true
    }

    /// Enables or disables rule at `idx`.
    pub fn set_enabled(&mut self, idx: usize, enabled: bool) -> bool {
        if let Some(r) = &mut self.rules[idx] {
            r.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Evaluates all rules against an event.
    ///
    /// RULE ARULE-01: Evaluation in ascending index order, first matching rule takes precedence.
    ///
    /// Returns `(action, rule_index)` or `(Log, usize::MAX)` if no rule matches.
    pub fn evaluate(
        &mut self,
        pid: u32,
        uid: u32,
        syscall_nr: u32,
        category: AuditCategory,
        outcome: u8,
    ) -> (RuleAction, usize) {
        // Sort by priority (find matching rule with highest priority)
        let mut best_priority = u8::MAX;
        let mut best_action = RuleAction::Log;
        let mut best_idx = usize::MAX;

        for i in 0..MAX_RULES {
            if let Some(r) = &mut self.rules[i] {
                if r.matches(pid, uid, syscall_nr, category, outcome) {
                    if r.priority < best_priority {
                        best_priority = r.priority;
                        best_action = r.action;
                        best_idx = i;
                    }
                }
            }
        }

        // Increment match counter of winning rule
        if best_idx < MAX_RULES {
            if let Some(r) = &mut self.rules[best_idx] {
                r.match_count = r.match_count.wrapping_add(1);
            }
        }

        (best_action, best_idx)
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global rule set
// ─────────────────────────────────────────────────────────────────────────────

static GLOBAL_RULES: spin::Mutex<RuleSet> = spin::Mutex::new(RuleSet::new());
static RULE_EVALS: AtomicU64 = AtomicU64::new(0);
static RULE_MATCHES: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Adds a global rule.
pub fn add_global_rule(rule: AuditRule) -> Result<usize, ()> {
    GLOBAL_RULES.lock().add_rule(rule)
}

/// Removes a global rule.
pub fn remove_global_rule(idx: usize) -> bool {
    GLOBAL_RULES.lock().remove_rule(idx)
}

/// Evaluates global rules against an event.
pub fn evaluate_global(
    pid: u32,
    uid: u32,
    syscall_nr: u32,
    category: AuditCategory,
    outcome: u8,
) -> RuleAction {
    RULE_EVALS.fetch_add(1, Ordering::Relaxed);
    let (action, idx) = GLOBAL_RULES
        .lock()
        .evaluate(pid, uid, syscall_nr, category, outcome);
    if idx != usize::MAX {
        RULE_MATCHES.fetch_add(1, Ordering::Relaxed);
    }
    action
}

#[derive(Debug, Clone, Copy)]
pub struct RuleStats {
    pub evaluations: u64,
    pub matches: u64,
    pub rule_count: usize,
}

pub fn rule_stats() -> RuleStats {
    RuleStats {
        evaluations: RULE_EVALS.load(Ordering::Relaxed),
        matches: RULE_MATCHES.load(Ordering::Relaxed),
        rule_count: GLOBAL_RULES.lock().count(),
    }
}
