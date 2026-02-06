//! Gestion des processus enfants

use crate::error::ProcessError;
use super::Pid;

/// Handle sur un processus enfant
pub struct Child {
    pid: Pid,
}

impl Child {
    /// Crée un nouveau handle (usage interne)
    pub(crate) const fn new(pid: Pid) -> Self {
        Self { pid }
    }

    /// Retourne l'ID du processus
    pub const fn id(&self) -> Pid {
        self.pid
    }

    /// Attend que le processus se termine
    pub fn wait(&self) -> Result<ExitStatus, ProcessError> {
        #[cfg(feature = "test_mode")]
        {
            Ok(ExitStatus::exited(0))
        }

        #[cfg(not(feature = "test_mode"))]
        {
            use crate::syscall::process::wait;

            unsafe {
                let mut status: i32 = 0;
                let _pid = wait(self.pid, &mut status as *mut i32)?;
                Ok(ExitStatus::from_raw(status))
            }
        }
    }

    /// Attend avec timeout (si disponible)
    ///
    /// Note: Cette implémentation actuelle ne supporte pas vraiment le timeout
    /// et se comporte comme wait(). Une vraie implémentation nécessiterait:
    /// - Un syscall waitpid avec timeout
    /// - Un mécanisme de polling non-bloquant
    pub fn wait_timeout(&self, _timeout: core::time::Duration) -> Result<Option<ExitStatus>, ProcessError> {
        self.wait().map(Some)
    }

    /// Tue le processus
    pub fn kill(&self) -> Result<(), ProcessError> {
        #[cfg(feature = "test_mode")]
        {
            Ok(())
        }

        #[cfg(not(feature = "test_mode"))]
        {
            use crate::syscall::process::kill;
            unsafe {
                kill(self.pid, 9).map_err(|e| e.into()) // SIGKILL
            }
        }
    }

    /// Essaie de récupérer le status sans bloquer
    ///
    /// Note: Cette implémentation retourne toujours None.
    /// Une vraie implémentation nécessiterait un syscall waitpid non-bloquant (WNOHANG).
    pub fn try_wait(&self) -> Result<Option<ExitStatus>, ProcessError> {
        // Retourne None car on ne peut pas vérifier sans bloquer
        Ok(None)
    }
}

impl core::fmt::Debug for Child {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Child")
            .field("pid", &self.pid)
            .finish()
    }
}

/// Status de sortie d'un processus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    code: i32,
    signal: Option<i32>,
}

impl ExitStatus {
    /// Crée un status depuis un code brut
    ///
    /// Sur les systèmes POSIX, le code contient à la fois le code de sortie et le signal:
    /// - Si WIFEXITED: code de sortie dans les 8 bits de poids faible
    /// - Si WIFSIGNALED: numéro de signal dans les 7 bits de poids faible
    ///
    /// Cette implémentation simplifiée traite le code comme un code de sortie direct.
    pub const fn from_raw(code: i32) -> Self {
        // Pour une implémentation complète, parser selon les macros POSIX:
        // - WIFEXITED(code): (code & 0x7F) == 0
        // - WEXITSTATUS(code): (code >> 8) & 0xFF
        // - WIFSIGNALED(code): ((code & 0x7F) + 1) >> 1 > 0
        // - WTERMSIG(code): code & 0x7F
        
        // Implémentation simplifiée: on considère que c'est un code de sortie
        Self {
            code,
            signal: None,
        }
    }

    /// Crée un status de sortie normale
    pub const fn exited(code: i32) -> Self {
        Self {
            code,
            signal: None,
        }
    }

    /// Crée un status de sortie par signal
    pub const fn signaled(signal: i32) -> Self {
        Self {
            code: -1,
            signal: Some(signal),
        }
    }

    /// Retourne true si le processus s'est terminé normalement
    pub const fn success(&self) -> bool {
        self.code == 0 && self.signal.is_none()
    }

    /// Retourne le code de sortie si disponible
    pub const fn code(&self) -> Option<i32> {
        if self.signal.is_none() {
            Some(self.code)
        } else {
            None
        }
    }

    /// Retourne le signal si le processus a été tué par un signal
    pub const fn signal(&self) -> Option<i32> {
        self.signal
    }
}

impl core::fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.signal {
            Some(sig) => write!(f, "killed by signal {}", sig),
            None => write!(f, "exited with code {}", self.code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_status() {
        let status = ExitStatus::exited(0);
        assert!(status.success());
        assert_eq!(status.code(), Some(0));
        assert_eq!(status.signal(), None);

        let status = ExitStatus::exited(1);
        assert!(!status.success());
        assert_eq!(status.code(), Some(1));

        let status = ExitStatus::signaled(9);
        assert!(!status.success());
        assert_eq!(status.code(), None);
        assert_eq!(status.signal(), Some(9));
    }
}
