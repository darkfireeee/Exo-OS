// libs/exo_std/src/process.rs
//! Gestion des processus
//!
//! Ce module fournit des API pour créer, gérer et contrôler des processus.

use crate::Result;
use crate::syscall::process as sys;

/// ID de processus
pub type Pid = sys::Pid;

/// Quitte le processus actuel avec le code donné
///
/// Cette fonction ne retourne jamais.
///
/// # Exemple
/// ```no_run
/// use exo_std::process;
///
/// process::exit(0); // Exit success
/// ```
#[inline]
pub fn exit(code: i32) -> ! {
    unsafe { sys::exit(code) }
}

/// Retourne l'ID du processus actuel
///
/// # Exemple
/// ```no_run
/// use exo_std::process;
///
/// let pid = process::id();
/// println!("PID: {}", pid);
/// ```
#[inline]
pub fn id() -> Pid {
    sys::getpid()
}

/// Crée un nouveau processus (fork)
///
/// Retourne 0 dans le processus enfant, le PID de l'enfant dans le parent.
///
/// # Exemple
/// ```no_run
/// use exo_std::process;
///
/// match process::fork().unwrap() {
///     0 => {
///         // Code enfant
///         println!("Je suis l'enfant");
///         process::exit(0);
///     }
///     child_pid => {
///         // Code parent
///         println!("Enfant créé: {}", child_pid);
///     }
/// }
/// ```
#[inline]
pub fn fork() -> Result<Pid> {
    unsafe { sys::fork() }
}

/// Attend la fin d'un processus enfant
///
/// Bloque jusqu'à ce que le processus spécifié se termine.
/// Retourne le PID et le code de sortie.
///
/// # Exemple
/// ```no_run
/// use exo_std::process;
///
/// let child_pid = process::fork().unwrap();
/// if child_pid == 0 {
///     process::exit(42);
/// }
///
/// let (pid, status) = process::wait(child_pid).unwrap();
/// println!("Enfant {} terminé avec status {}", pid, status);
/// ```
pub fn wait(pid: Pid) -> Result<(Pid, i32)> {
    let mut status = 0i32;
    unsafe {
        let waited_pid = sys::wait(pid, &mut status as *mut i32)?;
        Ok((waited_pid, status))
    }
}

/// Attend n'importe quel enfant
///
/// Équivalent à wait(-1) en POSIX.
#[inline]
pub fn wait_any() -> Result<(Pid, i32)> {
    wait(!0) // PID max = wait any
}

/// Envoie un signal à un processus
///
/// # Safety
/// L'appelant doit avoir les permissions pour envoyer le signal.
///
/// # Signaux Communs
/// - 9: SIGKILL (termine immédiatement)
/// - 15: SIGTERM (demande de terminaison)
/// - 2: SIGINT (interruption, Ctrl+C)
pub fn kill(pid: Pid, signal: i32) -> Result<()> {
    unsafe { sys::kill(pid, signal) }
}

/// Builder pour créer des processus avec configuration
#[derive(Debug)]
pub struct Command {
    program: alloc::string::String,
    args: alloc::vec::Vec<alloc::string::String>,
    env: alloc::vec::Vec<(alloc::string::String, alloc::string::String)>,
}

impl Command {
    /// Crée un nouveau Command pour le programme donné
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::process::Command;
    ///
    /// let output = Command::new("/bin/ls")
    ///     .arg("-la")
    ///     .arg("/tmp")
    ///     .spawn()
    ///     .unwrap();
    /// ```
    pub fn new<S: AsRef<str>>(program: S) -> Self {
        Self {
            program: alloc::string::String::from(program.as_ref()),
            args: alloc::vec::Vec::new(),
            env: alloc::vec::Vec::new(),
        }
    }
    
    /// Ajoute un argument
    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.args.push(alloc::string::String::from(arg.as_ref()));
        self
    }
    
    /// Ajoute plusieurs arguments
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }
    
    /// Ajoute une variable d'environnement
    pub fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.env.push((
            alloc::string::String::from(key.as_ref()),
            alloc::string::String::from(value.as_ref()),
        ));
        self
    }
    
    /// Lance le processus
    ///
    /// Retourne un Child qui peut être attendu.
    pub fn spawn(&self) -> Result<Child> {
        let pid = fork()?;
        
        if pid == 0 {
            // Dans l'enfant: exec
            // TODO: implémenter exec avec args et env
            // Pour l'instant, juste exit
            exit(0);
        } else {
            // Dans le parent
            Ok(Child { pid })
        }
    }
    
    /// Lance et attend la fin du processus
    pub fn status(&self) -> Result<ExitStatus> {
        let child = self.spawn()?;
        child.wait()
    }
}

/// Handle vers un processus enfant
#[derive(Debug)]
pub struct Child {
    pid: Pid,
}

impl Child {
    /// Retourne le PID du processus
    #[inline]
    pub fn id(&self) -> Pid {
        self.pid
    }
    
    /// Attend que le processus se termine
    pub fn wait(self) -> Result<ExitStatus> {
        let (_, status) = wait(self.pid)?;
        Ok(ExitStatus { code: status })
    }
    
    /// Tue le processus (SIGKILL)
    pub fn kill(&mut self) -> Result<()> {
        kill(self.pid, 9)
    }
    
    /// Tente d'obtenir le status sans bloquer
    ///
    /// Retourne None si le processus est toujours en cours.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        // TODO: implémenter waitpid non-bloquant
        // Pour l'instant, retourne None
        Ok(None)
    }
}

/// Status de sortie d'un processus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    code: i32,
}

impl ExitStatus {
    /// Retourne le code de sortie si disponible
    #[inline]
    pub fn code(&self) -> Option<i32> {
        Some(self.code)
    }
    
    /// Vérifie si le processus a réussi (code 0)
    #[inline]
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_process_id() {
        let pid = id();
        assert!(pid > 0);
    }
    
    #[test]
    fn test_command_builder() {
        let cmd = Command::new("/bin/test")
            .arg("--flag")
            .arg("value");
        
        assert_eq!(cmd.program, "/bin/test");
        assert_eq!(cmd.args.len(), 2);
    }
}
