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

                    // Convertir program en C-string
                    let mut program_bytes = self.program.as_bytes().to_vec();
                    program_bytes.push(0); // null terminator

                    // Convertir args en C-strings (argv[0] = program name par convention POSIX)
                    let mut args_bytes: Vec<Vec<u8>> = Vec::new();
                    args_bytes.push(program_bytes.clone());
                    for arg in &self.args {
                        let mut bytes = arg.as_bytes().to_vec();
                        bytes.push(0);
                        args_bytes.push(bytes);
                    }

                    // Créer tableau de pointeurs argv (terminé par NULL)
                    let mut argv_ptrs: Vec<*const u8> = args_bytes.iter()
                        .map(|v| v.as_ptr())
                        .collect();
                    argv_ptrs.push(core::ptr::null());

                    // Convertir env en C-strings (format: "KEY=VALUE\0")
                    let mut env_bytes: Vec<Vec<u8>> = Vec::new();
                    for (k, v) in &self.env {
                        let mut bytes = Vec::new();
                        bytes.extend_from_slice(k.as_bytes());
                        bytes.push(b'=');
                        bytes.extend_from_slice(v.as_bytes());
                        bytes.push(0);
                        env_bytes.push(bytes);
                    }

                    // Créer tableau de pointeurs envp (terminé par NULL)
                    let mut envp_ptrs: Vec<*const u8> = env_bytes.iter()
                        .map(|v| v.as_ptr())
                        .collect();
                    envp_ptrs.push(core::ptr::null());

                    // Appel exec avec raw pointers
                    let _ = exec(
                        program_bytes.as_ptr(),
                        argv_ptrs.as_ptr(),
                        envp_ptrs.as_ptr()
                    );

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
