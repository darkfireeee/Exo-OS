// libs/exo_std/src/process/mod.rs
pub mod spawn;
pub mod command;
pub mod exit;

pub use spawn::{spawn, spawn_with_env, Child};
pub use command::{Command, Stdio};
pub use exit::{exit, abort};

/// ID de processus
pub type Pid = u64;

/// Statut d'un processus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Running,
    Sleeping,
    Stopped,
    Zombie,
    Dead,
}

/// Informations sur un processus
pub struct ProcessInfo {
    pub pid: Pid,
    pub parent_pid: Pid,
    pub status: ProcessStatus,
    pub name: alloc::string::String,
    pub cpu_usage: f32,
    pub memory_usage: usize,
}

/// Obtient les informations sur le processus courant
pub fn current() -> ProcessInfo {
    ProcessInfo {
        pid: sys_getpid(),
        parent_pid: sys_getppid(),
        status: ProcessStatus::Running,
        name: sys_get_process_name(),
        cpu_usage: 0.0,
        memory_usage: sys_get_memory_usage(),
    }
}

/// Obtient les informations sur un processus par son PID
pub fn info(pid: Pid) -> Option<ProcessInfo> {
    sys_get_process_info(pid)
}

/// Liste des processus
pub fn list() -> alloc::vec::Vec<ProcessInfo> {
    sys_list_processes()
}

// Appels système
fn sys_getpid() -> Pid {
    #[cfg(feature = "test_mode")]
    {
        1
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_getpid() -> Pid;
            }
            sys_getpid()
        }
    }
}

fn sys_getppid() -> Pid {
    #[cfg(feature = "test_mode")]
    {
        0
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_getppid() -> Pid;
            }
            sys_getppid()
        }
    }
}

fn sys_get_process_name() -> alloc::string::String {
    #[cfg(feature = "test_mode")]
    {
        "test_process".to_string()
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        let mut name = [0u8; 256];
        unsafe {
            extern "C" {
                fn sys_get_process_name(buf: *mut u8, len: usize) -> usize;
            }
            let len = sys_get_process_name(name.as_mut_ptr(), name.len());
            alloc::string::String::from_utf8_lossy(&name[..len]).into_owned()
        }
    }
}

fn sys_get_memory_usage() -> usize {
    #[cfg(feature = "test_mode")]
    {
        4096
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_get_memory_usage() -> usize;
            }
            sys_get_memory_usage()
        }
    }
}

fn sys_get_process_info(pid: Pid) -> Option<ProcessInfo> {
    #[cfg(feature = "test_mode")]
    {
        Some(ProcessInfo {
            pid,
            parent_pid: pid - 1,
            status: ProcessStatus::Running,
            name: format!("process_{}", pid),
            cpu_usage: 1.0,
            memory_usage: 4096 * pid as usize,
        })
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        let mut info = core::mem::MaybeUninit::<ProcessInfo>::uninit();
        unsafe {
            extern "C" {
                fn sys_get_process_info(pid: Pid, info: *mut ProcessInfo) -> i32;
            }
            if sys_get_process_info(pid, info.as_mut_ptr()) == 0 {
                Some(info.assume_init())
            } else {
                None
            }
        }
    }
}

fn sys_list_processes() -> alloc::vec::Vec<ProcessInfo> {
    #[cfg(feature = "test_mode")]
    {
        vec![
            ProcessInfo {
                pid: 1,
                parent_pid: 0,
                status: ProcessStatus::Running,
                name: "init".to_string(),
                cpu_usage: 0.1,
                memory_usage: 4096,
            },
            ProcessInfo {
                pid: 2,
                parent_pid: 1,
                status: ProcessStatus::Running,
                name: "shell".to_string(),
                cpu_usage: 1.5,
                memory_usage: 8192,
            },
        ]
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // Implémentation réelle avec allocation dynamique
        let mut count = 0;
        unsafe {
            extern "C" {
                fn sys_get_process_count() -> usize;
                fn sys_list_processes(buf: *mut ProcessInfo, count: usize) -> usize;
            }
            count = sys_get_process_count();
        }
        
        let mut processes = alloc::vec::Vec::with_capacity(count);
        unsafe {
            processes.set_len(count);
            sys_list_processes(processes.as_mut_ptr(), count);
            processes.set_len(count.min(processes.len()));
        }
        
        processes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_process() {
        let info = current();
        assert_eq!(info.pid, 1); // En mode test
        assert_eq!(info.name, "test_process");
    }

    #[test]
    fn test_process_list() {
        let processes = list();
        assert!(!processes.is_empty());
        assert_eq!(processes[0].pid, 1);
        assert_eq!(processes[0].name, "init");
    }
}