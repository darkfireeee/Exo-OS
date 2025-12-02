//! Container Isolation
//!
//! Lightweight containers with resource isolation

use super::{Namespace, NamespaceType, SandboxPolicy};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Container Configuration
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub name: String,
    pub namespaces: Vec<NamespaceType>,
    pub root_path: String,
    pub sandbox_policy: SandboxPolicy,
    pub env: Vec<(String, String)>,
}

impl ContainerConfig {
    pub fn new(name: String) -> Self {
        Self {
            name,
            namespaces: vec![
                NamespaceType::Pid,
                NamespaceType::Mount,
                NamespaceType::Network,
            ],
            root_path: String::from("/"),
            sandbox_policy: SandboxPolicy::strict(),
            env: Vec::new(),
        }
    }

    pub fn with_all_namespaces(mut self) -> Self {
        self.namespaces = vec![
            NamespaceType::Pid,
            NamespaceType::Mount,
            NamespaceType::Network,
            NamespaceType::Ipc,
            NamespaceType::User,
        ];
        self
    }
}

/// Container Instance
pub struct Container {
    pub config: ContainerConfig,
    pub pid: u32,
    pub namespaces: Vec<Namespace>,
    pub state: ContainerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Stopped,
}

impl Container {
    /// Create a new container
    pub fn new(config: ContainerConfig) -> Result<Self, &'static str> {
        let namespaces = config
            .namespaces
            .iter()
            .map(|&ns_type| Namespace::new(ns_type))
            .collect();

        Ok(Self {
            config,
            pid: 0,
            namespaces,
            state: ContainerState::Created,
        })
    }

    /// Start the container
    pub fn start(&mut self) -> Result<(), &'static str> {
        if self.state != ContainerState::Created {
            return Err("Container already started");
        }

        // In production:
        // 1. Create namespaces
        // 2. Setup root filesystem
        // 3. Apply cgroups limits
        // 4. Fork process into container
        // 5. Apply seccomp filter

        self.state = ContainerState::Running;
        Ok(())
    }

    /// Stop the container
    pub fn stop(&mut self) -> Result<(), &'static str> {
        if self.state != ContainerState::Running {
            return Err("Container not running");
        }

        // In production: send SIGTERM, wait, then SIGKILL if needed

        self.state = ContainerState::Stopped;
        Ok(())
    }

    /// Pause the container
    pub fn pause(&mut self) -> Result<(), &'static str> {
        if self.state != ContainerState::Running {
            return Err("Container not running");
        }

        // In production: freeze cgroup

        self.state = ContainerState::Paused;
        Ok(())
    }

    /// Resume the container
    pub fn resume(&mut self) -> Result<(), &'static str> {
        if self.state != ContainerState::Paused {
            return Err("Container not paused");
        }

        // In production: thaw cgroup

        self.state = ContainerState::Running;
        Ok(())
    }
}
