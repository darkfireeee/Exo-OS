// libs/exo_std/src/process/spawn.rs
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use super::Pid;

/// Structure représentant un processus enfant
pub struct Child {
    pid: Pid,
    status: Option<ExitStatus>,
}

/// Statut de sortie d'un processus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus(i32);

impl ExitStatus {
    /// Retourne le code de sortie
    pub fn code(&self) -> i32 {
        self.0
    }
    
    /// Retourne si le processus s'est terminé avec succès
    pub fn success(&self) -> bool {
        self.0 == 0
    }
}

impl Child {
    /// Attends que l'enfant se termine
    pub fn wait(&mut self) -> Result<ExitStatus, super::super::io::IoError> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        
        let status = sys_waitpid(self.pid)?;
        self.status = Some(status);
        Ok(status)
    }
    
    /// Envoie un signal au processus enfant
    pub fn kill(&mut self) -> Result<(), super::super::io::IoError> {
        sys_kill(self.pid)
    }
}

/// Spawne un nouveau processus
///
/// # Arguments
/// * `entry_point` - Point d'entrée du nouveau processus
///
/// # Retour
/// `Child` handle sur succès, erreur sinon
pub fn spawn<F>(entry_point: F) -> Result<Child, super::super::io::IoError>
where
    F: FnOnce() + Send + 'static,
{
    // Créer un ID unique pour la fonction
    static ID_COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    
    // Allouer la fonction sur le tas
    let func = alloc::boxed::Box::new(entry_point);
    let raw_func = Box::into_raw(func) as *mut ();
    
    let pid = sys_spawn(raw_func, id)?;
    
    Ok(Child {
        pid,
        status: None,
    })
}

/// Spawne un nouveau processus avec variables d'environnement
///
/// # Arguments
/// * `entry_point` - Point d'entrée du nouveau processus
/// * `env_vars` - Variables d'environnement
///
/// # Retour
/// `Child` handle sur succès, erreur sinon
pub fn spawn_with_env<F>(
    entry_point: F,
    env_vars: Vec<(String, String)>,
) -> Result<Child, super::super::io::IoError>
where
    F: FnOnce() + Send + 'static,
{
    // Créer un ID unique pour la fonction
    static ID_COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    
    // Allouer la fonction sur le tas
    let func = alloc::boxed::Box::new(entry_point);
    let raw_func = Box::into_raw(func) as *mut ();
    
    // Alloue les variables d'environnement
    let env_ptr = Box::into_raw(env_vars.into_boxed_slice()) as *mut ();
    
    let pid = sys_spawn_with_env(raw_func, env_ptr, id)?;
    
    // Les ressources seront libérées par le noyau après fork
    core::mem::forget(env_ptr);
    
    Ok(Child {
        pid,
        status: None,
    })
}

// Appels système
fn sys_spawn(func: *mut (), id: u64) -> Result<Pid, super::super::io::IoError> {
    #[cfg(feature = "test_mode")]
    {
        Ok(id as Pid + 100) // PID artificiel pour les tests
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_spawn(func: *mut (), id: u64) -> i64;
            }
            let result = sys_spawn(func, id);
            if result < 0 {
                Err(super::super::io::IoError::Other)
            } else {
                Ok(result as Pid)
            }
        }
    }
}

fn sys_spawn_with_env(
    func: *mut (),
    env: *mut (),
    id: u64,
) -> Result<Pid, super::super::io::IoError> {
    #[cfg(feature = "test_mode")]
    {
        Ok(id as Pid + 200) // PID artificiel pour les tests
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_spawn_with_env(func: *mut (), env: *mut (), id: u64) -> i64;
            }
            let result = sys_spawn_with_env(func, env, id);
            if result < 0 {
                Err(super::super::io::IoError::Other)
            } else {
                Ok(result as Pid)
            }
        }
    }
}

fn sys_waitpid(pid: Pid) -> Result<ExitStatus, super::super::io::IoError> {
    #[cfg(feature = "test_mode")]
    {
        Ok(ExitStatus(0)) // Succès pour les tests
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_waitpid(pid: Pid, status: *mut i32) -> i64;
            }
            let mut status = 0;
            let result = sys_waitpid(pid, &mut status);
            if result < 0 {
                Err(super::super::io::IoError::Other)
            } else {
                Ok(ExitStatus(status))
            }
        }
    }
}

fn sys_kill(pid: Pid) -> Result<(), super::super::io::IoError> {
    #[cfg(feature = "test_mode")]
    {
        Ok(())
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_kill(pid: Pid, signal: i32) -> i32;
            }
            let result = sys_kill(pid, 9); // SIGKILL
            if result != 0 {
                Err(super::super::io::IoError::Other)
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_spawn_basic() {
        static RAN: AtomicBool = AtomicBool::new(false);
        
        let child = spawn(|| {
            RAN.store(true, Ordering::SeqCst);
        }).unwrap();
        
        child.wait().unwrap();
        assert!(RAN.load(Ordering::SeqCst));
    }
}