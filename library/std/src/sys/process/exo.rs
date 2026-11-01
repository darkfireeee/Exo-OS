use super::CommandEnvs;
use super::env::CommandEnv;
use crate::ffi::{OsStr, OsString};
pub use crate::ffi::OsString as EnvKey;
use crate::io::{self, Read};
use crate::num::NonZeroI32;
use crate::os::exo::ffi::OsStrExt;
use crate::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use crate::path::Path;
use crate::process::StdioPipes;
use crate::sys::fd::FileDesc;
use crate::sys::fs::File;
use crate::sys::pal::runtime;
use crate::sys::pal::syscall::{self, O_CLOEXEC, WNOHANG};
use crate::sys::pal::{cstr_bytes, map_exo_err};
use crate::sys::{AsInner, FromInner, IntoInner};
use crate::vec::Vec;
use crate::fmt;

pub enum Stdio {
    Inherit,
    Null,
    MakePipe,
    Fd(FileDesc),
}

impl Stdio {
    fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Fd(fd) => Ok(Self::Fd(FileDesc::from_inner(fd.as_inner().try_clone()?))),
            Self::Inherit => Ok(Self::Inherit),
            Self::Null => Ok(Self::Null),
            Self::MakePipe => Ok(Self::MakePipe),
        }
    }
}

#[derive(Default)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    env: CommandEnv,
}

impl Command {
    pub fn new(program: &OsStr) -> Command {
        Command {
            program: program.to_owned(),
            args: Vec::new(),
            cwd: None,
            stdin: None,
            stdout: None,
            stderr: None,
            env: CommandEnv::default(),
        }
    }

    pub fn arg(&mut self, arg: &OsStr) {
        self.args.push(arg.to_owned());
    }

    pub fn env_mut(&mut self) -> &mut CommandEnv {
        &mut self.env
    }

    pub fn cwd(&mut self, dir: &OsStr) {
        self.cwd = Some(dir.to_owned());
    }

    pub fn stdin(&mut self, stdin: Stdio) {
        self.stdin = Some(stdin);
    }

    pub fn stdout(&mut self, stdout: Stdio) {
        self.stdout = Some(stdout);
    }

    pub fn stderr(&mut self, stderr: Stdio) {
        self.stderr = Some(stderr);
    }

    pub fn get_program(&self) -> &OsStr {
        &self.program
    }

    pub fn get_args(&self) -> CommandArgs<'_> {
        CommandArgs { iter: self.args.iter() }
    }

    pub fn get_envs(&self) -> CommandEnvs<'_> {
        self.env.iter()
    }

    pub fn get_env_clear(&self) -> bool {
        self.env.does_clear()
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_ref().map(Path::new)
    }

    pub fn spawn(
        &mut self,
        default: Stdio,
        needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        let stdin_cfg = if let Some(s) = self.stdin.as_ref() {
            s.try_clone()?
        } else if needs_stdin {
            default.try_clone()?
        } else {
            Stdio::Null
        };
        let stdout_cfg = if let Some(s) = self.stdout.as_ref() {
            s.try_clone()?
        } else {
            default.try_clone()?
        };
        let stderr_cfg = if let Some(s) = self.stderr.as_ref() {
            s.try_clone()?
        } else {
            default.try_clone()?
        };

        let mut exec_fail_pipe = [-1i32; 2];
        syscall::pipe2(&mut exec_fail_pipe, O_CLOEXEC).map_err(map_exo_err)?;

        let mut stdin_pipe = [-1i32; 2];
        if matches!(stdin_cfg, Stdio::MakePipe) {
            if let Err(e) = syscall::pipe2(&mut stdin_pipe, 0) {
                let _ = syscall::close(exec_fail_pipe[0]);
                let _ = syscall::close(exec_fail_pipe[1]);
                return Err(map_exo_err(e));
            }
        }

        let mut stdout_pipe = [-1i32; 2];
        if matches!(stdout_cfg, Stdio::MakePipe) {
            if let Err(e) = syscall::pipe2(&mut stdout_pipe, 0) {
                let _ = syscall::close(exec_fail_pipe[0]);
                let _ = syscall::close(exec_fail_pipe[1]);
                if stdin_pipe[0] >= 0 {
                    let _ = syscall::close(stdin_pipe[0]);
                    let _ = syscall::close(stdin_pipe[1]);
                }
                return Err(map_exo_err(e));
            }
        }

        let mut stderr_pipe = [-1i32; 2];
        if matches!(stderr_cfg, Stdio::MakePipe) {
            if let Err(e) = syscall::pipe2(&mut stderr_pipe, 0) {
                let _ = syscall::close(exec_fail_pipe[0]);
                let _ = syscall::close(exec_fail_pipe[1]);
                if stdin_pipe[0] >= 0 {
                    let _ = syscall::close(stdin_pipe[0]);
                    let _ = syscall::close(stdin_pipe[1]);
                }
                if stdout_pipe[0] >= 0 {
                    let _ = syscall::close(stdout_pipe[0]);
                    let _ = syscall::close(stdout_pipe[1]);
                }
                return Err(map_exo_err(e));
            }
        }

        // Pre-allocate all C-strings before fork() so child never touches heap.
        let prog_c = cstr_bytes(self.program.as_bytes())?;
        let mut argv_storage: Vec<Vec<u8>> = Vec::with_capacity(1 + self.args.len());
        argv_storage.push(prog_c.clone());
        for a in &self.args {
            argv_storage.push(cstr_bytes(a.as_bytes())?);
        }
        let mut argv_ptrs: Vec<*const u8> = argv_storage.iter().map(|v| v.as_ptr()).collect();
        argv_ptrs.push(core::ptr::null());

        let captured_env = self.env.capture();
        let mut env_storage: Vec<Vec<u8>> = Vec::with_capacity(captured_env.len());
        for (k, v) in captured_env {
            let kb = k.as_bytes();
            let vb = v.as_bytes();
            let mut entry = Vec::with_capacity(kb.len() + 1 + vb.len() + 1);
            entry.extend_from_slice(kb);
            entry.push(b'=');
            entry.extend_from_slice(vb);
            entry.push(0);
            env_storage.push(entry);
        }
        let mut envp_ptrs: Vec<*const u8> = env_storage.iter().map(|v| v.as_ptr()).collect();
        envp_ptrs.push(core::ptr::null());

        let cwd_c = match self.cwd.as_ref() {
            Some(dir) => Some(cstr_bytes(dir.as_bytes())?),
            None => None,
        };

        let child_stdin_fd = match &stdin_cfg {
            Stdio::Fd(fd) => Some(fd.as_raw_fd()),
            _ => None,
        };
        let child_stdout_fd = match &stdout_cfg {
            Stdio::Fd(fd) => Some(fd.as_raw_fd()),
            _ => None,
        };
        let child_stderr_fd = match &stderr_cfg {
            Stdio::Fd(fd) => Some(fd.as_raw_fd()),
            _ => None,
        };

        let pid = match runtime::fork() {
            Ok(p) => p as i32,
            Err(errno) => {
                let _ = syscall::close(exec_fail_pipe[0]);
                let _ = syscall::close(exec_fail_pipe[1]);
                if stdin_pipe[0] >= 0 {
                    let _ = syscall::close(stdin_pipe[0]);
                    let _ = syscall::close(stdin_pipe[1]);
                }
                if stdout_pipe[0] >= 0 {
                    let _ = syscall::close(stdout_pipe[0]);
                    let _ = syscall::close(stdout_pipe[1]);
                }
                if stderr_pipe[0] >= 0 {
                    let _ = syscall::close(stderr_pipe[0]);
                    let _ = syscall::close(stderr_pipe[1]);
                }
                return Err(io::Error::from_raw_os_error(errno));
            }
        };

        if pid == 0 {
            let _ = syscall::close(exec_fail_pipe[0]);

            if stdin_pipe[0] >= 0 {
                let _ = syscall::close(stdin_pipe[1]);
                let _ = syscall::dup2(stdin_pipe[0], 0);
                let _ = syscall::close(stdin_pipe[0]);
            } else if let Some(fd) = child_stdin_fd {
                let _ = syscall::dup2(fd, 0);
            }

            if stdout_pipe[1] >= 0 {
                let _ = syscall::close(stdout_pipe[0]);
                let _ = syscall::dup2(stdout_pipe[1], 1);
                let _ = syscall::close(stdout_pipe[1]);
            } else if let Some(fd) = child_stdout_fd {
                let _ = syscall::dup2(fd, 1);
            }

            if stderr_pipe[1] >= 0 {
                let _ = syscall::close(stderr_pipe[0]);
                let _ = syscall::dup2(stderr_pipe[1], 2);
                let _ = syscall::close(stderr_pipe[1]);
            } else if let Some(fd) = child_stderr_fd {
                let _ = syscall::dup2(fd, 2);
            }

            if let Some(ref cwd_bytes) = cwd_c {
                let _ = syscall::chdir(cwd_bytes);
            }

            let _ = runtime::sigprocmask_0syscall(runtime::SIG_SETMASK, Some(0), None);

            let rc = unsafe {
                runtime::execve(prog_c.as_ptr(), argv_ptrs.as_ptr(), envp_ptrs.as_ptr())
            };
            let err_code: i32 = match rc {
                Ok(()) => 0,
                Err(code) => code,
            };
            let bytes = err_code.to_ne_bytes();
            let _ = syscall::write(exec_fail_pipe[1], &bytes);
            let _ = syscall::close(exec_fail_pipe[1]);
            syscall::exit_group(127);
        }

        // Parent process
        let _ = syscall::close(exec_fail_pipe[1]);
        if stdin_pipe[0] >= 0 {
            let _ = syscall::close(stdin_pipe[0]);
        }
        if stdout_pipe[1] >= 0 {
            let _ = syscall::close(stdout_pipe[1]);
        }
        if stderr_pipe[1] >= 0 {
            let _ = syscall::close(stderr_pipe[1]);
        }

        let mut err_buf = [0u8; 4];
        let n = syscall::read(exec_fail_pipe[0], &mut err_buf).unwrap_or(0);
        let _ = syscall::close(exec_fail_pipe[0]);

        if n == 4 {
            let child_errno = i32::from_ne_bytes(err_buf);
            let mut dummy_status = 0i32;
            let _ = syscall::wait4(pid, &mut dummy_status, 0);
            if stdin_pipe[1] >= 0 {
                let _ = syscall::close(stdin_pipe[1]);
            }
            if stdout_pipe[0] >= 0 {
                let _ = syscall::close(stdout_pipe[0]);
            }
            if stderr_pipe[0] >= 0 {
                let _ = syscall::close(stderr_pipe[0]);
            }
            return Err(io::Error::from_raw_os_error(child_errno));
        }

        let pipes = StdioPipes {
            stdin: if stdin_pipe[1] >= 0 {
                Some(unsafe { ChildPipe::from_raw_fd(stdin_pipe[1]) })
            } else {
                None
            },
            stdout: if stdout_pipe[0] >= 0 {
                Some(unsafe { ChildPipe::from_raw_fd(stdout_pipe[0]) })
            } else {
                None
            },
            stderr: if stderr_pipe[0] >= 0 {
                Some(unsafe { ChildPipe::from_raw_fd(stderr_pipe[0]) })
            } else {
                None
            },
        };

        Ok((Process { pid, status: None }, pipes))
    }
}

impl From<FileDesc> for Stdio {
    fn from(fd: FileDesc) -> Stdio {
        Stdio::Fd(fd)
    }
}

impl From<File> for Stdio {
    fn from(file: File) -> Stdio {
        Stdio::Fd(file.into_inner())
    }
}

impl From<io::Stdout> for Stdio {
    fn from(_: io::Stdout) -> Stdio {
        Stdio::Fd(unsafe { FileDesc::from_raw_fd(1) })
    }
}

impl From<io::Stderr> for Stdio {
    fn from(_: io::Stderr) -> Stdio {
        Stdio::Fd(unsafe { FileDesc::from_raw_fd(2) })
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Command")
            .field("program", &self.program)
            .field("args", &self.args)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ExitStatus(i32);

impl ExitStatus {
    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        if self.code() == Some(0) {
            Ok(())
        } else {
            Err(ExitStatusError(*self))
        }
    }

    pub fn code(&self) -> Option<i32> {
        if (self.0 & 0x7f) == 0 {
            Some((self.0 >> 8) & 0xff)
        } else {
            None
        }
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = self.code() {
            write!(f, "exit status: {code}")
        } else {
            write!(f, "signal: {}", self.0 & 0x7f)
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitStatusError(ExitStatus);

impl Into<ExitStatus> for ExitStatusError {
    fn into(self) -> ExitStatus {
        self.0
    }
}

impl ExitStatusError {
    pub fn code(self) -> Option<NonZeroI32> {
        self.0.code().and_then(NonZeroI32::new)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitCode(i32);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);

    pub fn as_i32(&self) -> i32 {
        self.0
    }
}

impl From<u8> for ExitCode {
    fn from(code: u8) -> Self {
        Self(code as i32)
    }
}

pub struct Process {
    pid: i32,
    status: Option<ExitStatus>,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn kill(&mut self) -> io::Result<()> {
        syscall::kill(self.pid, 9).map_err(map_exo_err)
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(st) = self.status {
            return Ok(st);
        }
        let mut raw_status = 0i32;
        loop {
            match syscall::wait4(self.pid, &mut raw_status, 0) {
                Ok(_) => {
                    let st = ExitStatus(raw_status);
                    self.status = Some(st);
                    return Ok(st);
                }
                Err(e) if e == syscall::SysError::EINTR => continue,
                Err(e) => return Err(map_exo_err(e)),
            }
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(st) = self.status {
            return Ok(Some(st));
        }
        let mut raw_status = 0i32;
        match syscall::wait4(self.pid, &mut raw_status, WNOHANG) {
            Ok(0) => Ok(None),
            Ok(_) => {
                let st = ExitStatus(raw_status);
                self.status = Some(st);
                Ok(Some(st))
            }
            Err(e) => Err(map_exo_err(e)),
        }
    }
}

pub struct CommandArgs<'a> {
    iter: crate::slice::Iter<'a, OsString>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;
    fn next(&mut self) -> Option<&'a OsStr> {
        self.iter.next().map(|arg| arg.as_os_str())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandArgs<'a> {
    fn len(&self) -> usize {
        self.iter.len()
    }
    fn is_empty(&self) -> bool {
        self.iter.is_empty()
    }
}

impl<'a> fmt::Debug for CommandArgs<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.clone()).finish()
    }
}

pub type ChildPipe = crate::sys::pipe::Pipe;

pub fn read_output(
    out: ChildPipe,
    stdout: &mut Vec<u8>,
    err: ChildPipe,
    stderr: &mut Vec<u8>,
) -> io::Result<()> {
    let _ = (&out).read_to_end(stdout);
    let _ = (&err).read_to_end(stderr);
    Ok(())
}

pub fn getpid() -> u32 {
    syscall::getpid()
}
