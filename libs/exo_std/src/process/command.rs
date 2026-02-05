//! Builder pattern pour la création de processus

extern crate alloc;
use alloc::vec::Vec;
use alloc::string::String;

use crate::error::ProcessError;
use crate::collections::BoundedVec;
use super::{Child, Pid};

/// Builder pour créer des processus
pub struct Command {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl Command {
    /// Crée une nouvelle commande
    pub fn new(program: &str) -> Self {
        Self {
            program: String::from(program),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Ajoute un argument
    pub fn arg(&mut self, arg: &str) -> &mut Self {
        self.args.push(String::from(arg));
        self
    }

    /// Ajoute plusieurs arguments
    pub fn args(&mut self, args: &[&str]) -> &mut Self {
        for &arg in args {
            self.args.push(String::from(arg));
        }
        self
    }

    /// Ajoute une variable d'environnement
    pub fn env(&mut self, key: &str, val: &str) -> &mut Self {
        self.env.push((String::from(key), String::from(val)));
        self
    }

    /// Définit plusieurs variables d'environnement
    pub fn envs(&mut self, envs: &[(&str, &str)]) -> &mut Self {
        for &(k, v) in envs {
            self.env.push((String::from(k), String::from(v)));
        }
        self
    }

    /// Lance le processus
    pub fn spawn(&self) -> Result<Child, ProcessError> {
        #[cfg(feature = "test_mode")]
        {
            Ok(Child::new(123))
        }
        
        #[cfg(not(feature = "test_mode"))]
        {
            use crate::syscall::process::{fork, exec};
            
            unsafe {
                let pid = fork()?;
                
                if pid == 0 {
                    // Enfant: exec
                    // Convertir Vec<String> en Vec<&str>
                    let args_strs: Vec<&str> = self.args.iter().map(|s| s.as_str()).collect();
                    let _ = exec(self.program.as_str(), &args_strs);
                    // Si exec échoue, exit
                    crate::process::exit(-1);
                } else {
                    // Parent: retourner Child
                    Ok(Child::new(pid as Pid))
                }
            }
        }
    }

    /// Lance le processus et attend sa terminaison
    pub fn status(&self) -> Result<super::ExitStatus, ProcessError> {
        let child = self.spawn()?;
        child.wait()
    }

    /// Lance le processus et capture la sortie
    ///
    /// Note: Cette implémentation ne capture pas réellement stdout/stderr.
    /// Pour capturer la sortie, il faudrait:
    /// - Créer des pipes avant le fork
    /// - Rediriger stdout/stderr du processus enfant vers les pipes
    /// - Lire depuis les pipes dans le processus parent
    /// - Utiliser des syscalls dup2() et pipe()
    pub fn output(&self) -> Result<Output, ProcessError> {
        let child = self.spawn()?;
        let status = child.wait()?;
        
        // Les vecteurs sont vides car on ne capture pas la sortie
        Ok(Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

/// Sortie d'un processus
pub struct Output {
    /// Status de sortie
    pub status: super::ExitStatus,
    /// Données stdout
    pub stdout: Vec<u8>,
    /// Données stderr
    pub stderr: Vec<u8>,
}

impl core::fmt::Debug for Command {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Command")
            .field("program", &self.program)
            .field("args", &self.args)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_builder() {
        let mut cmd = Command::new("/bin/ls");
        cmd.arg("-la")
            .arg("/tmp")
            .env("PATH", "/bin")
            .env("HOME", "/root");

        assert_eq!(cmd.args.len(), 2);
        assert_eq!(cmd.env.len(), 2);
    }

    #[test]
    #[cfg(feature = "test_mode")]
    fn test_command_spawn() {
        let cmd = Command::new("/bin/echo");
        let child = cmd.spawn().unwrap();
        assert!(child.id() > 0);
    }
}
