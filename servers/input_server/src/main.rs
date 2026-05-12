#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicUsize, Ordering};
use exo_syscall_abi as syscall;

pub const INPUT_MSG_PUSH: u32 = 0x120;
pub const INPUT_MSG_POLL: u32 = 0x121;
pub const INPUT_MSG_HEARTBEAT: u32 = 0x122;
const INPUT_QUEUE_LEN: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputEventWire {
    pub device: u8,
    pub state: u8,
    pub code: u16,
    pub value: i16,
    pub ascii: u8,
    pub modifiers: u8,
    pub _pad: [u8; 4],
}

#[repr(C)]
struct InputRequest {
    sender_pid: u32,
    msg_type: u32,
    event: InputEventWire,
}

#[repr(C)]
struct InputReply {
    status: i64,
    event: InputEventWire,
    queue_depth: u32,
    _pad: [u8; 4],
}

struct InputQueue {
    events: [InputEventWire; INPUT_QUEUE_LEN],
    head: usize,
    tail: usize,
    len: usize,
}

impl InputQueue {
    const fn new() -> Self {
        Self {
            events: [InputEventWire {
                device: 0,
                state: 0,
                code: 0,
                value: 0,
                ascii: 0,
                modifiers: 0,
                _pad: [0; 4],
            }; INPUT_QUEUE_LEN],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    fn push(&mut self, event: InputEventWire) -> Result<(), i64> {
        if self.len == INPUT_QUEUE_LEN {
            return Err(syscall::ENOBUFS);
        }
        self.events[self.tail] = event;
        self.tail = (self.tail + 1) % INPUT_QUEUE_LEN;
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<InputEventWire> {
        if self.len == 0 {
            return None;
        }
        let event = self.events[self.head];
        self.head = (self.head + 1) % INPUT_QUEUE_LEN;
        self.len -= 1;
        Some(event)
    }
}

struct QueueCell(UnsafeCell<InputQueue>);

unsafe impl Sync for QueueCell {}

static QUEUE: QueueCell = QueueCell(UnsafeCell::new(InputQueue::new()));
static DROPPED: AtomicUsize = AtomicUsize::new(0);

#[inline]
fn queue_mut() -> &'static mut InputQueue {
    // SAFETY: input_server is a single-threaded event loop; all queue access is
    // serialized in `_start` before a reply is sent.
    unsafe { &mut *QUEUE.0.get() }
}

fn handle(req: &InputRequest) -> InputReply {
    match req.msg_type {
        INPUT_MSG_PUSH => {
            let status = match queue_mut().push(req.event) {
                Ok(()) => 0,
                Err(err) => {
                    DROPPED.fetch_add(1, Ordering::Relaxed);
                    err
                }
            };
            InputReply {
                status,
                event: InputEventWire::default(),
                queue_depth: queue_mut().len as u32,
                _pad: [0; 4],
            }
        }
        INPUT_MSG_POLL => {
            let event = queue_mut().pop();
            InputReply {
                status: if event.is_some() { 0 } else { syscall::EAGAIN },
                event: event.unwrap_or_default(),
                queue_depth: queue_mut().len as u32,
                _pad: [0; 4],
            }
        }
        INPUT_MSG_HEARTBEAT => InputReply {
            status: 0,
            event: InputEventWire::default(),
            queue_depth: queue_mut().len as u32,
            _pad: [0; 4],
        },
        _ => InputReply {
            status: syscall::EINVAL,
            event: InputEventWire::default(),
            queue_depth: queue_mut().len as u32,
            _pad: [0; 4],
        },
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let name = b"input_server";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            11,
        )
    };
    let mut req = InputRequest {
        sender_pid: 0,
        msg_type: 0,
        event: InputEventWire::default(),
    };
    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                11,
                &mut req as *mut InputRequest as u64,
                core::mem::size_of::<InputRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 5_000,
            )
        };
        if rc < 0 {
            continue;
        }
        let reply = handle(&req);
        let _ = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                req.sender_pid as u64,
                &reply as *const InputReply as u64,
                core::mem::size_of::<InputReply>() as u64,
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
