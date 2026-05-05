#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_tty::{LineDiscipline, LineEvent, Signal};

pub const TTY_MSG_INPUT_BYTE: u32 = 0x130;
pub const TTY_MSG_READ_LINE: u32 = 0x131;
pub const TTY_MSG_WRITE: u32 = 0x132;
pub const TTY_MSG_IOCTL: u32 = 0x133;
const LINE_OUT_MAX: usize = 256;

#[repr(C)]
struct TtyRequest {
    sender_pid: u32,
    msg_type: u32,
    a: u64,
    b: u64,
    data: [u8; LINE_OUT_MAX],
}

#[repr(C)]
struct TtyReply {
    status: i64,
    signal: u32,
    len: u32,
    data: [u8; LINE_OUT_MAX],
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

fn handle_input(byte: u8) -> TtyReply {
    let state = tty_mut();
    let event = state.line.input_byte(byte);
    match event {
        Some(LineEvent::LineReady { len }) => {
            let mut tmp = [0u8; LINE_OUT_MAX];
            let copied = state.line.take_line(&mut tmp).len();
            let n = core::cmp::min(copied, core::cmp::min(len, LINE_OUT_MAX));
            state.ready[..n].copy_from_slice(&tmp[..n]);
            state.ready_len = n;
            reply(0, 0, &[])
        }
        Some(LineEvent::Signal(Signal::Interrupt)) => reply(0, 2, &[]),
        Some(LineEvent::Signal(Signal::EndOfFile)) => reply(0, 4, &[]),
        Some(LineEvent::Echo(byte)) => reply(0, 0, &[byte]),
        Some(LineEvent::Backspace) => reply(0, 0, b"\x08 \x08"),
        None => reply(0, 0, &[]),
    }
}

fn handle_read_line() -> TtyReply {
    let state = tty_mut();
    let n = state.ready_len;
    if n == 0 {
        return reply(syscall::EAGAIN, 0, &[]);
    }
    let mut data = [0u8; LINE_OUT_MAX];
    data[..n].copy_from_slice(&state.ready[..n]);
    state.ready_len = 0;
    TtyReply {
        status: 0,
        signal: 0,
        len: n as u32,
        data,
    }
}

fn reply(status: i64, signal: u32, data: &[u8]) -> TtyReply {
    let mut out = [0u8; LINE_OUT_MAX];
    let n = core::cmp::min(data.len(), LINE_OUT_MAX);
    out[..n].copy_from_slice(&data[..n]);
    TtyReply {
        status,
        signal,
        len: n as u32,
        data: out,
    }
}

fn handle(req: &TtyRequest) -> TtyReply {
    match req.msg_type {
        TTY_MSG_INPUT_BYTE => handle_input(req.a as u8),
        TTY_MSG_READ_LINE => handle_read_line(),
        TTY_MSG_WRITE => reply(
            0,
            0,
            &req.data[..core::cmp::min(req.a as usize, LINE_OUT_MAX)],
        ),
        TTY_MSG_IOCTL => reply(0, 0, &[]),
        _ => reply(syscall::EINVAL, 0, &[]),
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let name = b"tty_server";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            12,
        )
    };
    let mut req = TtyRequest {
        sender_pid: 0,
        msg_type: 0,
        a: 0,
        b: 0,
        data: [0; LINE_OUT_MAX],
    };
    loop {
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_RECV,
                &mut req as *mut TtyRequest as u64,
                core::mem::size_of::<TtyRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 5_000,
            )
        };
        if rc < 0 {
            continue;
        }
        let response = handle(&req);
        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                req.sender_pid as u64,
                &response as *const TtyReply as u64,
                core::mem::size_of::<TtyReply>() as u64,
                0,
                0,
                0,
            )
        };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
