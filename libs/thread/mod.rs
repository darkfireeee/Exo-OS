// libs/exo_std/src/thread/mod.rs
pub mod spawn;
pub mod join;
pub mod local;

pub use spawn::{spawn, Builder};
pub use join::JoinHandle;
pub use local::{LocalKey, LocalStorage};

use core::sync::atomic::{AtomicU64, Ordering};

/// ID de thread
pub type Tid = u64;

/// Priorité de thread
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPriority {
    Idle,
    Low,
    Normal,
    High,
    Critical,
}

/// Obtient l'ID du thread courant
pub fn current_id() -> Tid {
    sys_get_current_tid()
}

/// Obtient le nom du thread courant
pub fn current_name() -> Option<&'static str> {
    sys_get_current_name()
}

/// Met le thread en pause pour la durée spécifiée
pub fn sleep(duration: core::time::Duration) {
    sys_sleep(duration);
}

// Appels système
fn sys_get_current_tid() -> Tid {
    #[cfg(feature = "test_mode")]
    {
        1
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_get_current_tid() -> Tid;
            }
            sys_get_current_tid()
        }
    }
}

fn sys_get_current_name() -> Option<&'static str> {
    #[cfg(feature = "test_mode")]
    {
        Some("main")
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_get_current_name() -> *const u8;
                fn sys_get_name_length() -> usize;
            }
            let ptr = sys_get_current_name();
            let len = sys_get_name_length();
            if ptr.is_null() || len == 0 {
                None
            } else {
                let slice = core::slice::from_raw_parts(ptr, len);
                core::str::from_utf8(slice).ok()
            }
        }
    }
}

fn sys_sleep(duration: core::time::Duration) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire ou utiliser std::thread::sleep si disponible
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep(secs: u64, nsecs: u32);
            }
            sys_sleep(duration.as_secs(), duration.subsec_nanos());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_thread() {
        assert_eq!(current_id(), 1); // En mode test
        assert_eq!(current_name(), Some("main"));
    }
    
    #[test]
    fn test_thread_sleep() {
        let start = crate::time::Instant::now();
        sleep(core::time::Duration::from_millis(10));
        let elapsed = start.elapsed();
        assert!(elapsed >= core::time::Duration::from_millis(10));
    }
}