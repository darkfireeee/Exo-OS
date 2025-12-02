//! Seccomp-like System Call Filtering
//!
//! BPF-style filtering for syscalls to enforce sandboxing policies

use alloc::vec;
use alloc::vec::Vec;

/// Seccomp Action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeccompAction {
    Allow, // Allow the syscall
    Deny,  // Deny with EPERM
    Kill,  // Kill the thread
    Trap,  // Send SIGSYS
    Trace, // Notify tracer (ptrace)
    Log,   // Allow but log
}

/// Seccomp Rule
#[derive(Debug, Clone)]
pub struct SeccompRule {
    pub syscall_nr: u64,
    pub action: SeccompAction,
    pub args: Vec<ArgConstraint>,
}

/// Argument Constraint
#[derive(Debug, Clone)]
pub struct ArgConstraint {
    pub arg_index: u8, // Which argument (0-5)
    pub operation: ArgOp,
    pub value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgOp {
    Eq,       // ==
    Ne,       // !=
    Lt,       // <
    Le,       // <=
    Gt,       // >
    Ge,       // >=
    MaskedEq, // (arg & mask) == value
}

/// Seccomp Filter
pub struct SeccompFilter {
    pub default_action: SeccompAction,
    pub rules: Vec<SeccompRule>,
}

impl SeccompFilter {
    /// Create a new filter with default action
    pub fn new(default_action: SeccompAction) -> Self {
        Self {
            default_action,
            rules: Vec::new(),
        }
    }

    /// Add a rule to the filter
    pub fn add_rule(&mut self, rule: SeccompRule) {
        self.rules.push(rule);
    }

    /// Check if a syscall is allowed
    pub fn check(&self, syscall_nr: u64, args: &[u64; 6]) -> SeccompAction {
        // Check rules in order
        for rule in &self.rules {
            if rule.syscall_nr == syscall_nr {
                // Check all arg constraints
                let mut all_match = true;
                for constraint in &rule.args {
                    let arg = args[constraint.arg_index as usize];
                    let matches = match constraint.operation {
                        ArgOp::Eq => arg == constraint.value,
                        ArgOp::Ne => arg != constraint.value,
                        ArgOp::Lt => arg < constraint.value,
                        ArgOp::Le => arg <= constraint.value,
                        ArgOp::Gt => arg > constraint.value,
                        ArgOp::Ge => arg >= constraint.value,
                        ArgOp::MaskedEq => (arg & constraint.value) == constraint.value,
                    };
                    if !matches {
                        all_match = false;
                        break;
                    }
                }

                if all_match {
                    return rule.action;
                }
            }
        }

        self.default_action
    }

    /// Create a strict filter (deny most syscalls)
    pub fn strict() -> Self {
        let mut filter = Self::new(SeccompAction::Deny);

        // Allow only essential syscalls
        // (In production: define actual syscall numbers)
        let allowed = vec![
            0,   // read
            1,   // write
            60,  // exit
            231, // exit_group
        ];

        for syscall in allowed {
            filter.add_rule(SeccompRule {
                syscall_nr: syscall,
                action: SeccompAction::Allow,
                args: Vec::new(),
            });
        }

        filter
    }

    /// Create a permissive filter (log but allow)
    pub fn permissive() -> Self {
        Self::new(SeccompAction::Log)
    }
}
