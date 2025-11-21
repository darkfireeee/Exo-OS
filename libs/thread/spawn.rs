// libs/exo_std/src/thread/spawn.rs
use core::sync::atomic::{AtomicU64, Ordering};
use alloc::boxed::Box;
use super::Tid;
use super::join::JoinHandle;
use crate::io::Result as IoResult;

/// Builder pour configurer un nouveau thread
pub struct Builder {
    name: Option<String>,
    stack_size: Option<usize>,
    priority: Option<super::ThreadPriority>,
}

impl Builder {
    /// Crée un nouveau builder avec les paramètres par défaut
    pub fn new() -> Self {
        Builder {
            name: None,
            stack_size: None,
            priority: None,
        }
    }
    
    /// Définit le nom du thread
    pub fn name<S: Into<String>>(&mut self, name: S) -> &mut Self {
        self.name = Some(name.into());
        self
    }
    
    /// Définit la taille de la pile du thread
    pub fn stack_size(&mut self, size: usize) -> &mut Self {
        self.stack_size = Some(size);
        self
    }
    
    /// Définit la priorité du thread
    pub fn priority(&mut self, priority: super::ThreadPriority) -> &mut Self {
        self.priority = Some(priority);
        self
    }
    
    /// Démarre un nouveau thread avec la fonction spécifiée
    pub fn spawn<F, T>(&mut self, f: F) -> IoResult<JoinHandle<T>>
    where
        F: FnOnce() -> T,
        F: Send + 'static,
        T: Send + 'static,
    {
        spawn_thread(f, self.name.clone(), self.stack_size, self.priority)
    }
}

/// Spawne un nouveau thread avec les paramètres par défaut
pub fn spawn<F, T>(f: F) -> IoResult<JoinHandle<T>>
where
    F: FnOnce() -> T,
    F: Send + 'static,
    T: Send + 'static,
{
    spawn_thread(f, None, None, None)
}

// Fonction interne pour spawner un thread
fn spawn_thread<F, T>(
    f: F,
    name: Option<String>,
    stack_size: Option<usize>,
    priority: Option<super::ThreadPriority>,
) -> IoResult<JoinHandle<T>>
where
    F: FnOnce() -> T,
    F: Send + 'static,
    T: Send + 'static,
{
    // Créer un ID unique pour le thread
    static THREAD_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
    let thread_id = THREAD_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    
    // Allouer la fonction sur le tas
    let func = Box::new(f);
    let raw_func = Box::into_raw(func) as *mut ();
    
    // Allouer le nom si présent
    let raw_name = if let Some(name) = name {
        let name_box = Box::new(name.into_bytes());
        Box::into_raw(name_box) as *mut u8
    } else {
        core::ptr::null_mut()
    };
    
    // Créer le thread
    let tid = sys_spawn_thread(
        raw_func,
        thread_id,
        raw_name,
        stack_size.unwrap_or(0),
        priority.map(priority_to_int).unwrap_or(0),
    )?;
    
    Ok(JoinHandle::new(tid, thread_id))
}

// Conversion de priorité
fn priority_to_int(priority: super::ThreadPriority) -> u32 {
    match priority {
        super::ThreadPriority::Idle => 0,
        super::ThreadPriority::Low => 1,
        super::ThreadPriority::Normal => 2,
        super::ThreadPriority::High => 3,
        super::ThreadPriority::Critical => 4,
    }
}

// Appels système
fn sys_spawn_thread(
    func: *mut (),
    id: u64,
    name: *mut u8,
    stack_size: usize,
    priority: u32,
) -> IoResult<Tid> {
    #[cfg(feature = "test_mode")]
    {
        Ok(id + 100) // TID artificiel pour les tests
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_spawn_thread(
                    func: *mut (),
                    id: u64,
                    name: *mut u8,
                    name_len: usize,
                    stack_size: usize,
                    priority: u32,
                ) -> i64;
            }
            
            let name_len = if name.is_null() {
                0
            } else {
                core::ffi::CStr::from_ptr(name as *const i8).to_bytes().len()
            };
            
            let result = sys_spawn_thread(func, id, name, name_len, stack_size, priority);
            if result < 0 {
                Err(crate::io::IoError::Other)
            } else {
                Ok(result as Tid)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    
    #[test]
    fn test_basic_spawn() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        
        let handle = spawn(|| {
            COUNTER.store(42, Ordering::SeqCst);
        }).unwrap();
        
        handle.join().unwrap();
        assert_eq!(COUNTER.load(Ordering::SeqCst), 42);
    }
    
    #[test]
    fn test_builder() {
        let mut builder = Builder::new();
        builder.name("test_thread").stack_size(1024 * 1024);
        
        let handle = builder.spawn(|| {
            123
        }).unwrap();
        
        let result = handle.join().unwrap();
        assert_eq!(result, 123);
    }
    
    #[test]
    fn test_return_value() {
        let handle = spawn(|| {
            "Hello from thread!"
        }).unwrap();
        
        let result = handle.join().unwrap();
        assert_eq!(result, "Hello from thread!");
    }
}