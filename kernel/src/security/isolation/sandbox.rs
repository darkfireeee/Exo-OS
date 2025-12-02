//! Sandbox Implementation
//!
//! Lightweight sandboxing for untrusted code with resource limits

use super::seccomp::{SeccompAction, SeccompFilter};
use crate::security::capability::RightSet;
use alloc::vec;
use alloc::vec::Vec;

/// Sandbox policy
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// Allowed rights
    pub allowed_rights: RightSet,
    /// Blocked syscalls
    pub blocked_syscalls: Vec<u64>,
    /// Resource limits
    pub max_memory: usize,
    pub max_cpu_time: u64,
    pub max_file_descriptors: usize,
    pub max_threads: usize,
}

impl SandboxPolicy {
    /// Strict policy (minimal permissions)
    pub fn strict() -> Self {
        Self {
            allowed_rights: RightSet::new(), // No rights
            blocked_syscalls: Vec::new(),
            max_memory: 1024 * 1024, // 1MB
            max_cpu_time: 1000,      // 1 second
            max_file_descriptors: 10,
            max_threads: 1,
        }
    }

    /// Permissive policy (most permissions)
    pub fn permissive() -> Self {
        Self {
            allowed_rights: RightSet::all(),
            blocked_syscalls: Vec::new(),
            max_memory: usize::MAX,
            max_cpu_time: u64::MAX,
            max_file_descriptors: 1024,
            max_threads: 128,
        }
    }

    /// Network-restricted policy
    pub fn no_network() -> Self {
        let mut policy = Self::strict();
        // Block network-related syscalls
        policy.blocked_syscalls.extend_from_slice(&[
            41, // socket
            42, // connect
            43, // accept
            44, // sendto
            45, // recvfrom
            46, // sendmsg
            47, // recvmsg
        ]);
        policy
    }
}

/// Sandbox instance
pub struct Sandbox {
    pub policy: SandboxPolicy,
    pub violations: usize,
    pub seccomp_filter: Option<SeccompFilter>,
    pub resource_usage: ResourceUsage,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceUsage {
    pub memory_used: usize,
    pub cpu_time_used: u64,
    pub file_descriptors_open: usize,
    pub threads_active: usize,
}

impl Sandbox {
    /// Create a new sandbox
    pub fn new(policy: SandboxPolicy) -> Self {
        Self {
            policy,
            violations: 0,
            seccomp_filter: None,
            resource_usage: ResourceUsage::default(),
        }
    }

    /// With seccomp filter
    pub fn with_seccomp(mut self, filter: SeccompFilter) -> Self {
        self.seccomp_filter = Some(filter);
        self
    }

    /// Check if syscall is allowed
    pub fn check_syscall(&mut self, syscall_nr: u64, args: &[u64; 6]) -> bool {
        // Check policy blocklist
        if self.policy.blocked_syscalls.contains(&syscall_nr) {
            self.violations += 1;
            return false;
        }

        // Check seccomp filter
        if let Some(ref filter) = self.seccomp_filter {
            match filter.check(syscall_nr, args) {
                SeccompAction::Allow | SeccompAction::Log => true,
                SeccompAction::Deny | SeccompAction::Kill | SeccompAction::Trap => {
                    self.violations += 1;
                    false
                }
                SeccompAction::Trace => true, // Allow but traced
            }
        } else {
            true
        }
    }

    /// Check resource limits
    pub fn check_resource_limits(&self) -> Result<(), &'static str> {
        if self.resource_usage.memory_used > self.policy.max_memory {
            return Err("Memory limit exceeded");
        }

        if self.resource_usage.cpu_time_used > self.policy.max_cpu_time {
            return Err("CPU time limit exceeded");
        }

        if self.resource_usage.file_descriptors_open > self.policy.max_file_descriptors {
            return Err("File descriptor limit exceeded");
        }

        if self.resource_usage.threads_active > self.policy.max_threads {
            return Err("Thread limit exceeded");
        }

        Ok(())
    }

    /// Update resource usage
    pub fn update_usage(&mut self, usage: ResourceUsage) {
        self.resource_usage = usage;
    }
}
