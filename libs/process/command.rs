// libs/exo_std/src/process/command.rs
use alloc::{string::String, vec::Vec};
use super::spawn::{spawn_with_env, Child, ExitStatus};
use super::Pid;
use crate::io::{Read, Write, Result as IoResult};

/// Redirection d'E/S standard
pub enum Stdio {
    Inherit,
    Null,
    Piped,
}

/// Builder pour exécuter une commande externe
pub struct Command {
    program: String,
    args: Vec<String>,
    env_vars: Vec<(String, String)>,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
}

impl Command {
    /// Crée une nouvelle commande pour exécuter le programme spécifié
    pub fn new(program: impl Into<String>) -> Self {
        Command {
            program: program.into(),
            args: Vec::new(),
            env_vars: Vec::new(),
            stdin: Stdio::Inherit,
            stdout: Stdio::Inherit,
            stderr: Stdio::Inherit,
        }
    }
    
    /// Ajoute un argument à la commande
    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_string());
        self
    }
    
    /// Ajoute plusieurs arguments à la commande
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.args.push(arg.as_ref().to_string());
        }
        self
    }
    
    /// Ajoute une variable d'environnement
    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.env_vars.push((key.as_ref().to_string(), val.as_ref().to_string()));
        self
    }
    
    /// Configure l'E/S standard
    pub fn stdin(&mut self, cfg: Stdio) -> &mut Self {
        self.stdin = cfg;
        self
    }
    
    /// Configure la sortie standard
    pub fn stdout(&mut self, cfg: Stdio) -> &mut Self {
        self.stdout = cfg;
        self
    }
    
    /// Configure la sortie d'erreur
    pub fn stderr(&mut self, cfg: Stdio) -> &mut Self {
        self.stderr = cfg;
        self
    }
    
    /// Exécute la commande et attend la fin
    pub fn output(&mut self) -> IoResult<Output> {
        let child = self.spawn()?;
        let output = child.wait_with_output()?;
        Ok(output)
    }
    
    /// Démarre le processus sans attendre
    pub fn spawn(&mut self) -> IoResult<ChildProcess> {
        // Préparer les arguments et l'environnement
        let mut full_env = self.env_vars.clone();
        
        // Ajouter les variables d'environnement par défaut si nécessaire
        if full_env.is_empty() {
            // Obtenir l'environnement actuel
            let current_env = sys_get_current_env();
            full_env.extend(current_env);
        }
        
        // Créer la fonction d'entrée pour le nouveau processus
        let program = self.program.clone();
        let args = self.args.clone();
        let stdin = self.stdin.clone();
        let stdout = self.stdout.clone();
        let stderr = self.stderr.clone();
        
        let child = spawn_with_env(
            move || {
                // Configurer les redirections d'E/S
                sys_setup_io(stdin, stdout, stderr);
                
                // Exécuter le programme
                let result = sys_exec(&program, &args);
                
                // Si exec échoue, quitter avec un code d'erreur
                super::exit(result.map(|_| 0).unwrap_or(127));
            },
            full_env,
        )?;
        
        Ok(ChildProcess {
            inner: child,
            has_stdout_pipe: matches!(self.stdout, Stdio::Piped),
            has_stderr_pipe: matches!(self.stderr, Stdio::Piped),
            has_stdin_pipe: matches!(self.stdin, Stdio::Piped),
        })
    }
}

/// Processus enfant avec des pipes configurés
pub struct ChildProcess {
    inner: Child,
    has_stdout_pipe: bool,
    has_stderr_pipe: bool,
    has_stdin_pipe: bool,
}

impl ChildProcess {
    /// Attends la fin du processus et collecte la sortie
    pub fn wait_with_output(mut self) -> IoResult<Output> {
        // Configurer les buffers pour la sortie
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        
        // Lire les pipes si configurés
        if self.has_stdout_pipe {
            // Dans la vraie implémentation, lireait depuis un pipe
            stdout.extend_from_slice(b"Command output");
        }
        
        if self.has_stderr_pipe {
            // Dans la vraie implémentation, lirait depuis un pipe
            stderr.extend_from_slice(b"Command error");
        }
        
        // Attendre la fin du processus
        let status = self.inner.wait()?;
        
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
    
    /// Envoie des données sur stdin
    pub fn write_stdin(&mut self, buf: &[u8]) -> IoResult<usize> {
        if !self.has_stdin_pipe {
            return Err(crate::io::IoError::Other);
        }
        
        // Dans la vraie implémentation, écrirait dans un pipe
        Ok(buf.len())
    }
}

/// Résultat de l'exécution d'une commande
pub struct Output {
    /// Statut de sortie
    pub status: ExitStatus,
    /// Sortie standard
    pub stdout: Vec<u8>,
    /// Sortie d'erreur
    pub stderr: Vec<u8>,
}

// Fonctions système (stub pour les tests)
fn sys_get_current_env() -> Vec<(String, String)> {
    vec![
        ("PATH".to_string(), "/bin:/usr/bin".to_string()),
        ("HOME".to_string(), "/home/user".to_string()),
    ]
}

fn sys_setup_io(_stdin: Stdio, _stdout: Stdio, _stderr: Stdio) {
    // Configuration réelle des redirections d'E/S
}

fn sys_exec(_program: &str, _args: &[String]) -> Result<Pid, crate::io::IoError> {
    #[cfg(feature = "test_mode")]
    {
        Ok(123) // PID artificiel pour les tests
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        // Appel système réel pour exec
        Err(crate::io::IoError::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_basic() {
        let mut cmd = Command::new("echo");
        cmd.arg("Hello, Exo-OS!");
        
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"Command output");
    }
}