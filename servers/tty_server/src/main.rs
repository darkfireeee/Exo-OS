#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_tty::{LineDiscipline, LineEvent, Signal};

const LINE_OUT_MAX: usize = syscall::TTY_LINE_MAX;

#[inline]
fn boot_log(bytes: &[u8]) {
    write_console_all(bytes);
}

fn console_write(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    write_console_all(bytes);
}

fn write_console_all(bytes: &[u8]) {
    let mut done = 0usize;
    while done < bytes.len() {
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_WRITE,
                1,
                bytes[done..].as_ptr() as u64,
                (bytes.len() - done) as u64,
            )
        };
        if rc <= 0 {
            return;
        }
        done += rc as usize;
    }
}

fn exit_failed() -> ! {
    unsafe {
        let _ = syscall::syscall1(syscall::SYS_EXIT, 127);
        let _ = syscall::syscall1(syscall::SYS_EXIT_GROUP, 127);
    }
    loop {
        core::hint::spin_loop();
    }
}

struct TtyState {
    line: LineDiscipline,
    ready: [u8; LINE_OUT_MAX],
    ready_len: usize,
}

impl TtyState {
    const fn new() -> Self {
        Self {
            line: LineDiscipline::new(),
            ready: [0; LINE_OUT_MAX],
            ready_len: 0,
        }
    }
}

struct TtyCell(UnsafeCell<TtyState>);

unsafe impl Sync for TtyCell {}

static TTY: TtyCell = TtyCell(UnsafeCell::new(TtyState::new()));

#[inline]
fn tty_mut() -> &'static mut TtyState {
    // SAFETY: tty_server is a single-threaded event loop; requests are processed
    // to completion before the next IPC receive.
    unsafe { &mut *TTY.0.get() }
}

fn handle_input(byte: u8) -> syscall::TtyReply {
    let state = tty_mut();
    let event = state.line.input_byte(byte);
    match event {
        Some(LineEvent::LineReady { len }) => {
            let mut tmp = [0u8; LINE_OUT_MAX];
            let copied = state.line.take_line(&mut tmp).len();
            let n = core::cmp::min(copied, core::cmp::min(len, LINE_OUT_MAX));
            state.ready[..n].copy_from_slice(&tmp[..n]);
            state.ready_len = n;
            console_write(b"\n");
            reply(0, 0, &[])
        }
        Some(LineEvent::Signal(Signal::Interrupt)) => {
            console_write(b"^C\n");
            reply(0, 2, &[])
        }
        Some(LineEvent::Signal(Signal::EndOfFile)) => {
            console_write(b"^D\n");
            reply(0, 4, &[])
        }
        Some(LineEvent::Echo(byte)) => {
            console_write(&[byte]);
            reply(0, 0, &[byte])
        }
        Some(LineEvent::Backspace) => {
            console_write(b"\x08 \x08");
            reply(0, 0, b"\x08 \x08")
        }
        None => reply(0, 0, &[]),
    }
}

fn handle_read_line() -> syscall::TtyReply {
    let state = tty_mut();
    let n = state.ready_len;
    if n == 0 {
        return reply(syscall::EAGAIN, 0, &[]);
    }
    let mut data = [0u8; LINE_OUT_MAX];
    data[..n].copy_from_slice(&state.ready[..n]);
    state.ready_len = 0;
    syscall::TtyReply {
        status: 0,
        signal: 0,
        len: n as u32,
        data,
    }
}

fn reply(status: i64, signal: u32, data: &[u8]) -> syscall::TtyReply {
    let mut out = [0u8; LINE_OUT_MAX];
    let n = core::cmp::min(data.len(), LINE_OUT_MAX);
    out[..n].copy_from_slice(&data[..n]);
    syscall::TtyReply {
        status,
        signal,
        len: n as u32,
        data: out,
    }
}

fn handle(req: &syscall::TtyRequest) -> syscall::TtyReply {
    match req.msg_type {
        syscall::TTY_MSG_INPUT_BYTE => handle_input(req.a as u8),
        syscall::TTY_MSG_READ_LINE => handle_read_line(),
        syscall::TTY_MSG_WRITE => {
            let n = core::cmp::min(req.a as usize, LINE_OUT_MAX);
            console_write(&req.data[..n]);
            reply(0, 0, &req.data[..n])
        }
        syscall::TTY_MSG_IOCTL => reply(0, 0, &[]),
        _ => reply(syscall::EINVAL, 0, &[]),
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let name = b"tty_server";
    boot_log(b"tty_server: boot\n");
    let register_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            syscall::TTY_SERVER_ENDPOINT,
        )
    };
    if register_rc < 0 {
        boot_log(b"tty_server: register failed\n");
        exit_failed();
    }
    boot_log(b"tty_server: registered\n");
    let mut req = syscall::TtyRequest::zeroed();
    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                syscall::TTY_SERVER_ENDPOINT,
                &mut req as *mut syscall::TtyRequest as u64,
                core::mem::size_of::<syscall::TtyRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 5_000,
            )
        };
        if rc < 0 {
            continue;
        }
        let response = handle(&req);
        let reply_endpoint = if req.reply_endpoint != 0 {
            req.reply_endpoint
        } else {
            req.sender_pid as u64
        };
        if reply_endpoint != 0 {
            let _ = unsafe {
                syscall::syscall6(
                    syscall::SYS_IPC_SEND,
                    reply_endpoint,
                    &response as *const syscall::TtyReply as u64,
                    core::mem::size_of::<syscall::TtyReply>() as u64,
                    0,
                    0,
                    0,
                )
            };
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
