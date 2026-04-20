//! # policy — IPC policy enforcement engine
//!
//! Maintains a table of allow/deny rules for IPC communication between
//! PID pairs. The policy evaluation engine checks each IPC request
//! against the table and returns the appropriate action.
//!
//! ## Policy rules
//! Each rule specifies:
//! - Source PID (or wildcard 0 = any)
//! - Destination PID (or wildcard 0 = any)
//! - Message type filter (or wildcard 0 = any)
//! - Action: Allow, Deny, AuditOnly, RateLimit
//! - Priority: higher priority rules override lower ones
//!
//! ## Evaluation order
//! Rules are evaluated from highest to lowest priority. The first
//! matching rule determines the outcome. If no rule matches, the
//! default policy (configurable) is applied.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of policy entries.
const MAX_POLICY_ENTRIES: usize = 64;

/// Wildcard PID — matches any source or destination.
pub const WILDCARD_PID: u32 = 0;

/// Wildcard message type — matches any IPC message type.
pub const WILDCARD_MSG_TYPE: u32 = 0;

/// Default rate limit (messages per second per PID pair).
const DEFAULT_RATE_LIMIT: u32 = 1000;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Policy action to take when a rule matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PolicyAction {
    /// Allow the IPC message to proceed.
    Allow = 0,
    /// Deny the IPC message — drop silently.
    Deny = 1,
    /// Allow the IPC message but log it for audit.
    AuditOnly = 2,
    /// Allow the IPC message but enforce a rate limit.
    RateLimit = 3,
}

impl PolicyAction {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Allow,
            1 => Self::Deny,
            2 => Self::AuditOnly,
            3 => Self::RateLimit,
            _ => Self::Deny,
        }
    }
}

/// A single IPC policy rule.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PolicyRule {
    /// Source PID (0 = wildcard — matches any source).
    pub src_pid: u32,
    /// Destination PID (0 = wildcard — matches any destination).
    pub dst_pid: u32,
    /// Message type filter (0 = wildcard — matches any message type).
    pub msg_type: u32,
    /// Action to take when this rule matches.
    pub action: u8,
    /// Priority of this rule (0–255, higher = evaluated first).
    pub priority: u8,
    /// Flags: bit 0 = active, bit 1 = bidirectional.
    pub flags: u8,
    /// Rate limit (messages/second) when action = RateLimit.
    pub rate_limit: u8,
    /// Rule ID for management (set on insertion).
    pub rule_id: u32,
}

impl Default for PolicyRule {
    fn default() -> Self {
        Self {
            src_pid: 0,
            dst_pid: 0,
            msg_type: 0,
            action: PolicyAction::Deny as u8,
            priority: 0,
            flags: 0,
            rate_limit: 0,
            rule_id: 0,
        }
    }
}

/// Result of policy evaluation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PolicyEvalResult {
    /// The action determined by policy evaluation.
    pub action: u8,
    /// The rule ID that matched (0 = default policy applied).
    pub matched_rule_id: u32,
    /// Rate limit value if action is RateLimit.
    pub rate_limit: u32,
    /// Whether the message should be audited.
    pub audit: u8,
}

impl Default for PolicyEvalResult {
    fn default() -> Self {
        Self {
            action: PolicyAction::Deny as u8,
            matched_rule_id: 0,
            rate_limit: 0,
            audit: 0,
        }
    }
}

/// Rate-limit tracking entry per (src, dst) pair.
struct RateLimitEntry {
    src_pid: u32,
    dst_pid: u32,
    count: u32,
    window_start: u64,
}

impl RateLimitEntry {
    const fn new() -> Self {
        Self {
            src_pid: 0,
            dst_pid: 0,
            count: 0,
            window_start: 0,
        }
    }
}

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

// ── Static storage ────────────────────────────────────────────────────────────

/// Policy table — max 64 entries.
static POLICY_TABLE: Mutex<[PolicyRule; MAX_POLICY_ENTRIES]> = Mutex::new(
    [PolicyRule::default(); MAX_POLICY_ENTRIES],
);

/// Number of active policy entries.
static POLICY_COUNT: AtomicU32 = AtomicU32::new(0);

/// Next rule ID counter.
static NEXT_RULE_ID: AtomicU32 = AtomicU32::new(1);

/// Default policy action when no rule matches.
static DEFAULT_POLICY: AtomicU8 = AtomicU8::new(PolicyAction::Deny as u8);

/// Rate-limit tracking table (max 32 concurrent pairs).
static RATE_LIMIT_TABLE: Mutex<[RateLimitEntry; 32]> = Mutex::new(
    [RateLimitEntry::new(); 32],
);

/// Statistics.
static TOTAL_EVALUATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOWED_BY_POLICY: AtomicU64 = AtomicU64::new(0);
static DENIED_BY_POLICY: AtomicU64 = AtomicU64::new(0);
static AUDITED_BY_POLICY: AtomicU64 = AtomicU64::new(0);
static RATE_LIMITED: AtomicU64 = AtomicU64::new(0);
static DEFAULT_APPLIED: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Check if a rule matches the given (src_pid, dst_pid, msg_type).
fn rule_matches(rule: &PolicyRule, src_pid: u32, dst_pid: u32, msg_type: u32) -> bool {
    // Rule must be active
    if rule.flags & 1 == 0 {
        return false;
    }

    let src_match = rule.src_pid == WILDCARD_PID || rule.src_pid == src_pid;
    let dst_match = rule.dst_pid == WILDCARD_PID || rule.dst_pid == dst_pid;
    let msg_match = rule.msg_type == WILDCARD_MSG_TYPE || rule.msg_type == msg_type;

    if src_match && dst_match && msg_match {
        return true;
    }

    // Check bidirectional flag — if set, also match reversed PID pair
    if rule.flags & 2 != 0 {
        let src_rev = rule.src_pid == WILDCARD_PID || rule.src_pid == dst_pid;
        let dst_rev = rule.dst_pid == WILDCARD_PID || rule.dst_pid == src_pid;
        return src_rev && dst_rev && msg_match;
    }

    false
}

/// Check rate limit for a (src_pid, dst_pid) pair.
/// Returns `true` if the rate limit is exceeded.
fn check_rate_limit(src_pid: u32, dst_pid: u32, limit: u32) -> bool {
    let mut table = RATE_LIMIT_TABLE.lock();
    let now = read_tsc();
    // ~1 second window at 3 GHz
    let window = 3_000_000_000u64;

    for i in 0..32 {
        if table[i].src_pid == src_pid && table[i].dst_pid == dst_pid {
            let elapsed = now.wrapping_sub(table[i].window_start);
            if elapsed > window {
                table[i].count = 1;
                table[i].window_start = now;
                return false;
            }
            table[i].count += 1;
            return table[i].count > limit;
        }
    }

    // New entry
    for i in 0..32 {
        if table[i].src_pid == 0 {
            table[i].src_pid = src_pid;
            table[i].dst_pid = dst_pid;
            table[i].count = 1;
            table[i].window_start = now;
            return false;
        }
    }

    // Evict oldest
    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..32 {
        if table[i].window_start < oldest_tsc {
            oldest_tsc = table[i].window_start;
            oldest_idx = i;
        }
    }
    table[oldest_idx].src_pid = src_pid;
    table[oldest_idx].dst_pid = dst_pid;
    table[oldest_idx].count = 1;
    table[oldest_idx].window_start = now;
    false
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Policy table handle — provides a typed interface to the static table.
pub struct PolicyTable;

impl PolicyTable {
    /// Evaluates the policy for the given IPC parameters.
    ///
    /// Scans the rule table from highest to lowest priority and returns
    /// the action of the first matching rule. If no rule matches, the
    /// default policy is applied.
    pub fn evaluate(&self, src_pid: u32, dst_pid: u32, msg_type: u32) -> PolicyEvalResult {
        evaluate_policy(src_pid, dst_pid, msg_type)
    }
}

/// Evaluates the IPC policy for a message from `src_pid` to `dst_pid`
/// with message type `msg_type`.
///
/// The evaluation proceeds as follows:
/// 1. Collect all matching rules
/// 2. Select the one with the highest priority
/// 3. If action is RateLimit, check the rate limit
/// 4. If no rule matches, apply the default policy
pub fn evaluate_policy(src_pid: u32, dst_pid: u32, msg_type: u32) -> PolicyEvalResult {
    TOTAL_EVALUATIONS.fetch_add(1, Ordering::Relaxed);

    let table = POLICY_TABLE.lock();
    let count = POLICY_COUNT.load(Ordering::Acquire) as usize;

    let mut best_match: Option<&PolicyRule> = None;
    let mut best_priority = 0u8;

    for i in 0..count.min(MAX_POLICY_ENTRIES) {
        let rule = &table[i];
        if rule_matches(rule, src_pid, dst_pid, msg_type) {
            if rule.priority > best_priority || best_match.is_none() {
                best_priority = rule.priority;
                best_match = Some(rule);
            }
        }
    }

    match best_match {
        Some(rule) => {
            let action = PolicyAction::from_u8(rule.action);
            let mut result = PolicyEvalResult {
                action: rule.action,
                matched_rule_id: rule.rule_id,
                rate_limit: 0,
                audit: if action == PolicyAction::AuditOnly { 1 } else { 0 },
            };

            match action {
                PolicyAction::Allow => {
                    ALLOWED_BY_POLICY.fetch_add(1, Ordering::Relaxed);
                }
                PolicyAction::Deny => {
                    DENIED_BY_POLICY.fetch_add(1, Ordering::Relaxed);
                }
                PolicyAction::AuditOnly => {
                    ALLOWED_BY_POLICY.fetch_add(1, Ordering::Relaxed);
                    AUDITED_BY_POLICY.fetch_add(1, Ordering::Relaxed);
                }
                PolicyAction::RateLimit => {
                    let limit = if rule.rate_limit > 0 {
                        rule.rate_limit as u32 * 10 // rate_limit field is in 10msg/s units
                    } else {
                        DEFAULT_RATE_LIMIT
                    };
                    result.rate_limit = limit;
                    if check_rate_limit(src_pid, dst_pid, limit) {
                        RATE_LIMITED.fetch_add(1, Ordering::Relaxed);
                        result.action = PolicyAction::Deny as u8;
                        DENIED_BY_POLICY.fetch_add(1, Ordering::Relaxed);
                    } else {
                        ALLOWED_BY_POLICY.fetch_add(1, Ordering::Relaxed);
                        AUDITED_BY_POLICY.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            result
        }
        None => {
            // No matching rule — apply default policy
            DEFAULT_APPLIED.fetch_add(1, Ordering::Relaxed);
            let default_action = PolicyAction::from_u8(DEFAULT_POLICY.load(Ordering::Acquire));
            match default_action {
                PolicyAction::Allow => ALLOWED_BY_POLICY.fetch_add(1, Ordering::Relaxed),
                PolicyAction::Deny => DENIED_BY_POLICY.fetch_add(1, Ordering::Relaxed),
                _ => {}
            }
            PolicyEvalResult {
                action: default_action as u8,
                matched_rule_id: 0,
                rate_limit: 0,
                audit: 0,
            }
        }
    }
}

/// Adds a policy rule to the table.
///
/// Returns the rule ID assigned to the new rule, or 0 if the table is full.
pub fn add_policy(
    src_pid: u32,
    dst_pid: u32,
    msg_type: u32,
    action: PolicyAction,
    priority: u8,
    bidirectional: bool,
    rate_limit: u8,
) -> u32 {
    let rule_id = NEXT_RULE_ID.fetch_add(1, Ordering::AcqRel);
    let mut flags = 1u8; // bit 0 = active
    if bidirectional {
        flags |= 2; // bit 1 = bidirectional
    }

    let rule = PolicyRule {
        src_pid,
        dst_pid,
        msg_type,
        action: action as u8,
        priority,
        flags,
        rate_limit,
        rule_id,
    };

    let mut table = POLICY_TABLE.lock();
    let count = POLICY_COUNT.load(Ordering::Acquire) as usize;

    if count < MAX_POLICY_ENTRIES {
        table[count] = rule;
        POLICY_COUNT.fetch_add(1, Ordering::Release);
        return rule_id;
    }

    // Find an inactive slot
    for i in 0..MAX_POLICY_ENTRIES {
        if table[i].flags & 1 == 0 {
            table[i] = rule;
            POLICY_COUNT.fetch_add(1, Ordering::Release);
            return rule_id;
        }
    }

    // Table full — find lowest priority rule to evict
    let mut lowest_idx = 0usize;
    let mut lowest_priority = 255u8;
    for i in 0..MAX_POLICY_ENTRIES {
        if table[i].priority < lowest_priority && table[i].priority < priority {
            lowest_priority = table[i].priority;
            lowest_idx = i;
        }
    }

    if lowest_priority < priority {
        table[lowest_idx] = rule;
        return rule_id;
    }

    // Cannot insert — all rules have higher or equal priority
    0
}

/// Removes a policy rule by rule ID.
///
/// Returns `true` if the rule was found and deactivated.
pub fn remove_policy(rule_id: u32) -> bool {
    let mut table = POLICY_TABLE.lock();
    let count = POLICY_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_POLICY_ENTRIES) {
        if table[i].rule_id == rule_id {
            table[i].flags = 0; // Mark inactive
            // Shift remaining entries to keep the table compact
            for j in i..(count - 1).min(MAX_POLICY_ENTRIES - 1) {
                table[j] = table[j + 1];
            }
            if count > 0 {
                table[count.min(MAX_POLICY_ENTRIES) - 1] = PolicyRule::default();
            }
            POLICY_COUNT.fetch_sub(1, Ordering::Release);
            return true;
        }
    }
    false
}

/// Looks up a policy rule by rule ID.
///
/// Returns a copy of the rule if found.
pub fn lookup_policy(rule_id: u32) -> Option<PolicyRule> {
    let table = POLICY_TABLE.lock();
    let count = POLICY_COUNT.load(Ordering::Acquire) as usize;

    for i in 0..count.min(MAX_POLICY_ENTRIES) {
        if table[i].rule_id == rule_id && table[i].flags & 1 != 0 {
            return Some(table[i]);
        }
    }
    None
}

/// Sets the default policy action (applied when no rule matches).
pub fn set_default_policy(action: PolicyAction) {
    DEFAULT_POLICY.store(action as u8, Ordering::Release);
}

/// Gets the current default policy action.
pub fn get_default_policy() -> PolicyAction {
    PolicyAction::from_u8(DEFAULT_POLICY.load(Ordering::Acquire))
}

/// Policy subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PolicyStats {
    pub total_evaluations: u64,
    pub allowed: u64,
    pub denied: u64,
    pub audited: u64,
    pub rate_limited: u64,
    pub default_applied: u64,
    pub active_rules: u32,
}

/// Collects policy statistics.
pub fn get_policy_stats() -> PolicyStats {
    PolicyStats {
        total_evaluations: TOTAL_EVALUATIONS.load(Ordering::Relaxed),
        allowed: ALLOWED_BY_POLICY.load(Ordering::Relaxed),
        denied: DENIED_BY_POLICY.load(Ordering::Relaxed),
        audited: AUDITED_BY_POLICY.load(Ordering::Relaxed),
        rate_limited: RATE_LIMITED.load(Ordering::Relaxed),
        default_applied: DEFAULT_APPLIED.load(Ordering::Relaxed),
        active_rules: POLICY_COUNT.load(Ordering::Relaxed),
    }
}

/// Enumerates all active policy rules into a caller-provided buffer.
///
/// Returns the number of rules written.
pub fn enumerate_policies(out: &mut [PolicyRule]) -> usize {
    let table = POLICY_TABLE.lock();
    let count = POLICY_COUNT.load(Ordering::Acquire) as usize;
    let written = count.min(out.len()).min(MAX_POLICY_ENTRIES);

    for i in 0..written {
        out[i] = table[i];
    }
    written
}

/// Resets the policy subsystem and installs default rules.
pub fn policy_init() {
    POLICY_COUNT.store(0, Ordering::Release);
    NEXT_RULE_ID.store(1, Ordering::Release);
    DEFAULT_POLICY.store(PolicyAction::Deny as u8, Ordering::Release);
    TOTAL_EVALUATIONS.store(0, Ordering::Release);
    ALLOWED_BY_POLICY.store(0, Ordering::Release);
    DENIED_BY_POLICY.store(0, Ordering::Release);
    AUDITED_BY_POLICY.store(0, Ordering::Release);
    RATE_LIMITED.store(0, Ordering::Release);
    DEFAULT_APPLIED.store(0, Ordering::Release);

    // Install default rules:
    // 1. Kernel (PID 0) can send to anyone — Allow, priority 255
    add_policy(0, WILDCARD_PID, WILDCARD_MSG_TYPE, PolicyAction::Allow, 255, false, 0);

    // 2. Init (PID 1) can send to anyone — Allow, priority 200
    add_policy(1, WILDCARD_PID, WILDCARD_MSG_TYPE, PolicyAction::Allow, 200, false, 0);

    // 3. Anyone can send to exo_shield — AuditOnly, priority 100
    add_policy(WILDCARD_PID, 5, WILDCARD_MSG_TYPE, PolicyAction::AuditOnly, 100, true, 0);
}
