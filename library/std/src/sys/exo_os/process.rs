//! `std::sys::exo_os::process` — Process spawning (`Command`, `Child`, `ExitStatus`, `Stdio`)
//! following `PROC-03..PROC-10`:
//! 1. `pipe2(&mut exec_fail_pipe, O_CLOEXEC)` (`SYS_PIPE2 = 293`)
//! 2. `ExoOs::fork()` (which calls `fflush_all()` before `SYS_CLONE = 56` per `PROC-04`)
//! 3. Child: `dup2` stdin/stdout/stderr, reset signal mask via `SignalTcb` 0-syscall,
//!    call `SYS_EXECVE = 59`, write `errno: i32` on `exec_fail_pipe[1]` and `_exit(127)` on failure.
//! 4. Parent: close `exec_fail_pipe[1]`, read `exec_fail_pipe[0]` (`0` bytes = exec succeeded,
//!    `4` bytes = child `execve` failed with `errno`), `wait4` (`SYS_WAIT4 = 61`) on `Child::wait`.

use crate::alloc_crate::string::{String, ToString};
use crate::alloc_crate::vec::Vec;
use exo_libc::platform::PalSignal;
use exo_libc::{ExoOs, Pal};
use exo_syscall::exofs::{dup2, pipe2, O_CLOEXEC};
use super::stdio::{Error, ErrorKind, Result};

/// Standard I/O configuration for a child process (`std::process::Stdio`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdio {
    Inherit,
    Null,
    Piped,
    Fd(i32),
}

/// Exit status of a child process (`std::process::ExitStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    raw: i32,
}

impl ExitStatus {
    #[inline]
    pub const fn from_raw(raw: i32) -> Self {
        Self { raw }
    }

    #[inline]
    pub const fn success(&self) -> bool {
        matches!(self.code(), Some(0))
    }

    #[inline]
    pub const fn code(&self) -> Option<i32> {
        if (self.raw & 0x7f) == 0 {
            Some((self.raw >> 8) & 0xff)
        } else {
            None
        }
    }

    #[inline]
    pub const fn signal(&self) -> Option<i32> {
        let sig = self.raw & 0x7f;
        if sig != 0 && sig != 0x7f {
            Some(sig)
        } else {
            None
        }
    }
}

/// Output of a finished child process (`std::process::Output`).
#[derive(Debug, Clone)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// A running or exited child process (`std::process::Child`).
#[derive(Debug)]
pub struct Child {
    pid: i32,
    status: Option<ExitStatus>,
    stdout_fd: Option<i32>,
    stderr_fd: Option<i32>,
}

impl Child {
    #[inline]
    pub const fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        if let Some(st) = self.status {
            return Ok(st);
        }
        let mut raw_status = 0i32;
        loop {
            match ExoOs::wait4(self.pid, &mut raw_status, 0) {
                Ok(_) => {
                    let st = ExitStatus::from_raw(raw_status);
                    self.status = Some(st);
                    return Ok(st);
                }
                Err(e) => {
                    let err = Error::from_raw_os_error(e.errno());
                    if err.kind() == ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }

    pub fn wait_with_output(mut self) -> Result<Output> {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(fd) = self.stdout_fd.take() {
            let mut chunk = [0u8; 512];
            while let Ok(n) = ExoOs::read(fd, &mut chunk) {
                if n == 0 {
                    break;
                }
                stdout.extend_from_slice(&chunk[..n]);
            }
            let _ = ExoOs::close(fd);
        }
        if let Some(fd) = self.stderr_fd.take() {
            let mut chunk = [0u8; 512];
            while let Ok(n) = ExoOs::read(fd, &mut chunk) {
                if n == 0 {
                    break;
                }
                stderr.extend_from_slice(&chunk[..n]);
            }
            let _ = ExoOs::close(fd);
        }
        let status = self.wait()?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    pub fn kill(&mut self) -> Result<()> {
        ExoOs::kill(self.pid, 9).map_err(|e| Error::from_raw_os_error(e.errno()))
    }
}

/// Process builder (`std::process::Command`).
#[derive(Debug, Clone)]
pub struct Command {
    program: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
}

impl Command {
    pub fn new(program: &str) -> Self {
        Self {
            program: program.to_string(),
            args: Vec::new(),
            envs: Vec::new(),
            stdin: Stdio::Inherit,
            stdout: Stdio::Inherit,
            stderr: Stdio::Inherit,
        }
    }

    pub fn arg(&mut self, arg: &str) -> &mut Self {
        self.args.push(arg.to_string());
        self
    }

    pub fn args<'a, I>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        for a in args {
            self.args.push(a.to_string());
        }
        self
    }

    pub fn env(&mut self, key: &str, val: &str) -> &mut Self {
        self.envs.push((key.to_string(), val.to_string()));
        self
    }

    pub fn stdin(&mut self, cfg: Stdio) -> &mut Self {
        self.stdin = cfg;
        self
    }

    pub fn stdout(&mut self, cfg: Stdio) -> &mut Self {
        self.stdout = cfg;
        self
    }

    pub fn stderr(&mut self, cfg: Stdio) -> &mut Self {
        self.stderr = cfg;
        self
    }

    /// Spawns the process following `PROC-05` (`pipe2(O_CLOEXEC)` + `fork` + `dup2` + `execve`).
    pub fn spawn(&mut self) -> Result<Child> {
        let mut exec_fail_pipe = [-1i32; 2];
        pipe2(&mut exec_fail_pipe, O_CLOEXEC)
            .map_err(|e| Error::from_raw_os_error(e.errno()))?;

        let mut stdout_pipe = [-1i32; 2];
        if self.stdout == Stdio::Piped {
            if let Err(e) = pipe2(&mut stdout_pipe, 0) {
                let _ = ExoOs::close(exec_fail_pipe[0]);
                let _ = ExoOs::close(exec_fail_pipe[1]);
                return Err(Error::from_raw_os_error(e.errno()));
            }
        }

        let mut stderr_pipe = [-1i32; 2];
        if self.stderr == Stdio::Piped {
            if let Err(e) = pipe2(&mut stderr_pipe, 0) {
                let _ = ExoOs::close(exec_fail_pipe[0]);
                let _ = ExoOs::close(exec_fail_pipe[1]);
                if stdout_pipe[0] >= 0 {
                    let _ = ExoOs::close(stdout_pipe[0]);
                    let _ = ExoOs::close(stdout_pipe[1]);
                }
                return Err(Error::from_raw_os_error(e.errno()));
            }
        }

        // Prepare NUL-terminated strings in the parent before fork() so child allocates nothing.
        let mut prog_c = self.program.as_bytes().to_vec();
        prog_c.push(0);

        let mut argv_storage: Vec<Vec<u8>> = Vec::with_capacity(1 + self.args.len());
        argv_storage.push(prog_c.clone());
        for a in &self.args {
            let mut v = a.as_bytes().to_vec();
            v.push(0);
            argv_storage.push(v);
        }
        let mut argv_ptrs: Vec<*const u8> = argv_storage.iter().map(|v| v.as_ptr()).collect();
        argv_ptrs.push(core::ptr::null());

        let mut env_storage: Vec<Vec<u8>> = Vec::with_capacity(self.envs.len());
        for (k, v) in &self.envs {
            let mut entry = Vec::with_capacity(k.len() + 1 + v.len() + 1);
            entry.extend_from_slice(k.as_bytes());
            entry.push(b'=');
            entry.extend_from_slice(v.as_bytes());
            entry.push(0);
            env_storage.push(entry);
        }
        let mut envp_ptrs: Vec<*const u8> = env_storage.iter().map(|v| v.as_ptr()).collect();
        envp_ptrs.push(core::ptr::null());

        let pid = match ExoOs::fork() {
            Ok(p) => p as i32,
            Err(e) => {
                let _ = ExoOs::close(exec_fail_pipe[0]);
                let _ = ExoOs::close(exec_fail_pipe[1]);
                if stdout_pipe[0] >= 0 {
                    let _ = ExoOs::close(stdout_pipe[0]);
                    let _ = ExoOs::close(stdout_pipe[1]);
                }
                if stderr_pipe[0] >= 0 {
                    let _ = ExoOs::close(stderr_pipe[0]);
                    let _ = ExoOs::close(stderr_pipe[1]);
                }
                return Err(Error::from_raw_os_error(e.errno()));
            }
        };

        if pid == 0 {
            // Child process
            let _ = ExoOs::close(exec_fail_pipe[0]);

            if let Stdio::Fd(fd) = self.stdin {
                let _ = dup2(fd, 0);
            }
            if self.stdout == Stdio::Piped {
                let _ = ExoOs::close(stdout_pipe[0]);
                let _ = dup2(stdout_pipe[1], 1);
                let _ = ExoOs::close(stdout_pipe[1]);
            } else if let Stdio::Fd(fd) = self.stdout {
                let _ = dup2(fd, 1);
            }
            if self.stderr == Stdio::Piped {
                let _ = ExoOs::close(stderr_pipe[0]);
                let _ = dup2(stderr_pipe[1], 2);
                let _ = ExoOs::close(stderr_pipe[1]);
            } else if let Stdio::Fd(fd) = self.stderr {
                let _ = dup2(fd, 2);
            }

            // Reset signal mask via 0-syscall SignalTcb fast-path
            let _ = ExoOs::sigprocmask(2, Some(0), None);

            let rc = unsafe { ExoOs::execve(prog_c.as_ptr(), argv_ptrs.as_ptr(), envp_ptrs.as_ptr()) };
            let err_code: i32 = match rc {
                Ok(()) => 2,
                Err(e) => e.errno(),
            };
            let _ = ExoOs::write(exec_fail_pipe[1], &err_code.to_ne_bytes());
            let _ = ExoOs::close(exec_fail_pipe[1]);
            ExoOs::exit(127);
        }

        // Parent process
        let _ = ExoOs::close(exec_fail_pipe[1]);
        if stdout_pipe[1] >= 0 {
            let _ = ExoOs::close(stdout_pipe[1]);
        }
        if stderr_pipe[1] >= 0 {
            let _ = ExoOs::close(stderr_pipe[1]);
        }

        let mut err_buf = [0u8; 4];
        let n = ExoOs::read(exec_fail_pipe[0], &mut err_buf).unwrap_or(0);
        let _ = ExoOs::close(exec_fail_pipe[0]);

        if n == 4 {
            let child_errno = i32::from_ne_bytes(err_buf);
            let mut _st = 0i32;
            let _ = ExoOs::wait4(pid, &mut _st, 0);
            if stdout_pipe[0] >= 0 {
                let _ = ExoOs::close(stdout_pipe[0]);
            }
            if stderr_pipe[0] >= 0 {
                let _ = ExoOs::close(stderr_pipe[0]);
            }
            return Err(Error::from_raw_os_error(child_errno));
        }

        Ok(Child {
            pid,
            status: None,
            stdout_fd: if stdout_pipe[0] >= 0 {
                Some(stdout_pipe[0])
            } else {
                None
            },
            stderr_fd: if stderr_pipe[0] >= 0 {
                Some(stderr_pipe[0])
            } else {
                None
            },
        })
    }

    pub fn status(&mut self) -> Result<ExitStatus> {
        self.spawn()?.wait()
    }

    pub fn output(&mut self) -> Result<Output> {
        self.stdout(Stdio::Piped);
        self.stderr(Stdio::Piped);
        self.spawn()?.wait_with_output()
    }
}

/// Terminates the current process with the specified exit code (`SYS_EXIT_GROUP = 231`).
pub fn exit(code: i32) -> ! {
    ExoOs::exit(code)
}

/// Returns the OS-assigned process identifier (`SYS_GETPID = 39`).
pub fn id() -> u32 {
    ExoOs::getpid()
}
