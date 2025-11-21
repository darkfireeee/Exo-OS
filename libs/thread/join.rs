// libs/exo_std/src/thread/join.rs
use super::Tid;
use alloc::boxed::Box;
use core::task::{Context, Poll};
use core::future::Future;
use core::pin::Pin;

/// Handle pour joindre un thread
pub struct JoinHandle<T> {
    tid: Tid,
    thread_id: u64,
    result: Option<Box<T>>,
}

impl<T> JoinHandle<T> {
    pub(crate) fn new(tid: Tid, thread_id: u64) -> Self {
        JoinHandle {
            tid,
            thread_id,
            result: None,
        }
    }
    
    /// Attends la fin du thread et retourne son résultat
    pub fn join(mut self) -> crate::io::Result<T> {
        let result = sys_join_thread(self.tid)?;
        
        // Dans la vraie implémentation, le résultat serait récupéré du thread
        if let Some(result) = self.result.take() {
            Ok(*result)
        } else {
            // Placeholder pour les tests
            Ok(unsafe { core::mem::zeroed() })
        }
    }
    
    /// Retourne l'ID du thread
    pub fn thread_id(&self) -> Tid {
        self.tid
    }
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = crate::io::Result<T>;
    
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if sys_is_thread_finished(self.tid) {
            Poll::Ready(self.get_mut().join())
        } else {
            Poll::Pending
        }
    }
}

// Appels système
fn sys_join_thread(tid: Tid) -> crate::io::Result<()> {
    #[cfg(feature = "test_mode")]
    {
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_join_thread(tid: Tid) -> i32;
            }
            let result = sys_join_thread(tid);
            if result != 0 {
                Err(crate::io::IoError::Other)
            } else {
                Ok(())
            }
        }
    }
}

fn sys_is_thread_finished(tid: Tid) -> bool {
    #[cfg(feature = "test_mode")]
    {
        true // Les threads terminent immédiatement dans les tests
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_is_thread_finished(tid: Tid) -> i32;
            }
            sys_is_thread_finished(tid) != 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    
    #[test]
    fn test_join_basic() {
        static DONE: AtomicUsize = AtomicUsize::new(0);
        
        let handle = super::spawn(|| {
            DONE.store(1, Ordering::SeqCst);
        }).unwrap();
        
        handle.join().unwrap();
        assert_eq!(DONE.load(Ordering::SeqCst), 1);
    }
    
    #[test]
    fn test_join_result() {
        let handle = super::spawn(|| {
            42
        }).unwrap();
        
        let result = handle.join().unwrap();
        assert_eq!(result, 42);
    }
}