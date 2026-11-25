//! # syscall/fs_bridge/special/tty_bridge.rs — Ring 0 <-> Ring 1 `tty_server` IPC bridge

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::ipc::core::types::{EndpointId, IpcError};
use crate::syscall::fs_bridge::*;
use crate::syscall::validation::{copy_from_user, copy_to_user};

/// FIX-V8: log E9 console mode only once before tty_server.
#[cfg(target_arch = "x86_64")]
pub static TTY_E9ONLY_LOGGED: AtomicBool = AtomicBool::new(false);

pub use exo_wire::tty::{TtyReplyWire, TtyRequestWire};

pub struct TtyInputBuffer {
    pub data: [u8; TTY_LINE_MAX],
    pub head: usize,
    pub len: usize,
}

impl TtyInputBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0; TTY_LINE_MAX],
            head: 0,
            len: 0,
        }
    }

    pub fn push_slice(&mut self, bytes: &[u8]) -> usize {
        if self.head > 0 && self.len > 0 {
            self.data.copy_within(self.head..self.head + self.len, 0);
        }
        self.head = 0;
        let space = TTY_LINE_MAX.saturating_sub(self.len);
        let n = bytes.len().min(space);
        if n != 0 {
            self.data[self.len..self.len + n].copy_from_slice(&bytes[..n]);
            self.len += n;
        }
        n
    }

    pub fn peek_into(&self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        if n != 0 {
            out[..n].copy_from_slice(&self.data[self.head..self.head + n]);
        }
        n
    }

    pub fn consume(&mut self, n: usize) {
        let taken = n.min(self.len);
        self.head += taken;
        self.len -= taken;
        if self.len == 0 {
            self.head = 0;
        }
    }

    pub fn pop_into(&mut self, out: &mut [u8]) -> usize {
        let n = self.peek_into(out);
        self.consume(n);
        n
    }
}

#[inline]
pub fn is_tty_handle(handle: u32) -> bool {
    handle == TTY_PTS0_HANDLE
}

#[inline]
pub fn is_tty_handle_u64(handle: u64) -> bool {
    handle == TTY_PTS0_HANDLE as u64
}

#[inline]
pub fn is_tty_path(path: &[u8]) -> bool {
    path == b"/dev/pts/0" || path == b"/dev/tty"
}

pub fn tty_endpoint() -> Result<EndpointId, FsBridgeError> {
    let cached = TTY_ENDPOINT_CACHE.load(Ordering::Acquire);
    if cached != 0 {
        if let Some(endpoint) = EndpointId::new(cached) {
            return Ok(endpoint);
        }
        TTY_ENDPOINT_CACHE.store(0, Ordering::Release);
    }

    let endpoint = crate::ipc::endpoint::lookup_endpoint(TTY_SERVER_ENDPOINT_NAME)
        .ok_or(FsBridgeError::NotReady)?;
    TTY_ENDPOINT_CACHE.store(endpoint.get(), Ordering::Release);
    Ok(endpoint)
}

pub fn tty_call(req: &TtyRequestWire) -> Result<TtyReplyWire, FsBridgeError> {
    let endpoint = tty_endpoint()?;
    let request = unsafe {
        core::slice::from_raw_parts(
            req as *const TtyRequestWire as *const u8,
            size_of::<TtyRequestWire>(),
        )
    };
    let mut reply_buf = [0u8; size_of::<TtyReplyWire>()];
    match crate::ipc::rpc::call_raw(endpoint, request, &mut reply_buf) {
        Ok(n) if n >= size_of::<TtyReplyWire>() => {
            Ok(unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const TtyReplyWire) })
        }
        Ok(_) => Err(FsBridgeError::Invalid),
        Err(err) => Err(ipc_to_fs_error(err)),
    }
}

pub fn tty_send(req: &TtyRequestWire) -> Result<(), FsBridgeError> {
    let endpoint = tty_endpoint()?;
    let request = unsafe {
        core::slice::from_raw_parts(
            req as *const TtyRequestWire as *const u8,
            size_of::<TtyRequestWire>(),
        )
    };
    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(TTY_SEND_TIMEOUT_NS);

    loop {
        match crate::ipc::channel::raw::try_send_raw_nowait(endpoint, request) {
            Ok(_) => return Ok(()),
            Err(IpcError::WouldBlock) | Err(IpcError::QueueFull) | Err(IpcError::Full) => {
                if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                    return Err(FsBridgeError::WouldBlock);
                }
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
            }
            Err(err) => return Err(ipc_to_fs_error(err)),
        }
    }
}

pub fn tty_write_bytes(pid: u32, bytes: &[u8]) -> Result<i64, FsBridgeError> {
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    crate::arch::x86_64::terminal::debug_write(bytes);
    let mut written = 0usize;
    while written < bytes.len() {
        let n = (bytes.len() - written).min(TTY_LINE_MAX);
        let mut req = TtyRequestWire::zeroed();
        req.sender_pid = pid;
        req.msg_type = TTY_MSG_WRITE;
        req.a = n as u64;
        req.data[..n].copy_from_slice(&bytes[written..written + n]);
        match tty_send(&req) {
            Ok(()) => {}
            Err(FsBridgeError::NotReady) => {
                #[cfg(target_arch = "x86_64")]
                {
                    if !TTY_E9ONLY_LOGGED.swap(true, Ordering::AcqRel) {
                        crate::arch::x86_64::terminal::debug_write(b"<TTY-E9ONLY>\n");
                    }
                    crate::arch::x86_64::terminal::debug_write(&bytes[written..]);
                }
                return Ok(bytes.len() as i64);
            }
            Err(e) => return Err(e),
        }
        written += n;
    }
    Ok(bytes.len() as i64)
}

/// TTY write without heap allocation: copies user memory slices
/// directly into stack-allocated TTY line requests (184 bytes max).
pub fn tty_write_from_user(pid: u32, buf_ptr: u64, count: usize) -> Result<i64, FsBridgeError> {
    let mut written = 0usize;
    while written < count {
        let n = (count - written).min(TTY_LINE_MAX);
        let mut req = TtyRequestWire::zeroed();
        req.sender_pid = pid;
        req.msg_type = TTY_MSG_WRITE;
        req.a = n as u64;
        let uptr = (buf_ptr as usize)
            .checked_add(written)
            .ok_or(FsBridgeError::Fault)? as *const u8;
        copy_from_user(req.data.as_mut_ptr(), uptr, n).map_err(|_| FsBridgeError::Fault)?;
        #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
        crate::arch::x86_64::terminal::debug_write(&req.data[..n]);

        match tty_send(&req) {
            Ok(()) => {}
            Err(FsBridgeError::NotReady) => {
                #[cfg(target_arch = "x86_64")]
                {
                    if !TTY_E9ONLY_LOGGED.swap(true, Ordering::AcqRel) {
                        crate::arch::x86_64::terminal::debug_write(b"<TTY-E9ONLY>\n");
                    }
                    crate::arch::x86_64::terminal::debug_write(&req.data[..n]);
                    let mut rem = count - written - n;
                    let mut cur = written + n;
                    while rem > 0 {
                        let c = rem.min(TTY_LINE_MAX);
                        let ptr = (buf_ptr as usize)
                            .checked_add(cur)
                            .ok_or(FsBridgeError::Fault)?
                            as *const u8;
                        let mut tmp = [0u8; TTY_LINE_MAX];
                        copy_from_user(tmp.as_mut_ptr(), ptr, c)
                            .map_err(|_| FsBridgeError::Fault)?;
                        crate::arch::x86_64::terminal::debug_write(&tmp[..c]);
                        cur += c;
                        rem -= c;
                    }
                }
                return Ok(count as i64);
            }
            Err(e) => return Err(e),
        }
        written += n;
    }
    Ok(count as i64)
}

static TTY_REFILL_LOCK: Mutex<()> = Mutex::new(());

fn tty_refill_stdin_locked(pid: u32) -> Result<(), FsBridgeError> {
    let mut req = TtyRequestWire::zeroed();
    req.sender_pid = pid;
    req.msg_type = TTY_MSG_READ_LINE;
    let reply = tty_call(&req)?;
    if reply.status < 0 {
        return Err(errno_to_fs_error(reply.status));
    }
    let n = (reply.len as usize).min(TTY_LINE_MAX);
    if n == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    TTY_STDIN.lock().push_slice(&reply.data[..n]);
    Ok(())
}

pub fn tty_refill_stdin(pid: u32) -> Result<(), FsBridgeError> {
    let _guard = TTY_REFILL_LOCK.lock();
    tty_refill_stdin_locked(pid)
}

pub fn tty_read_bytes(buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if count == 0 {
        return Ok(0);
    }
    let _guard = TTY_REFILL_LOCK.lock();
    let mut out = [0u8; TTY_LINE_MAX];
    let cap = count.min(TTY_LINE_MAX);
    let mut copied = TTY_STDIN.lock().peek_into(&mut out[..cap]);
    if copied == 0 {
        tty_refill_stdin_locked(pid)?;
        copied = TTY_STDIN.lock().peek_into(&mut out[..cap]);
    }
    if copied == 0 {
        return Err(FsBridgeError::WouldBlock);
    }
    copy_to_user(buf_ptr as *mut u8, out.as_ptr(), copied).map_err(|_| FsBridgeError::Fault)?;
    TTY_STDIN.lock().consume(copied);
    Ok(copied as i64)
}
