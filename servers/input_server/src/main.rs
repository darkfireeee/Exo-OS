#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use exo_syscall_abi as syscall;

const INPUT_QUEUE_LEN: usize = 128;

struct InputQueue {
    events: [syscall::InputEventWire; INPUT_QUEUE_LEN],
    head: usize,
    tail: usize,
    len: usize,
}

impl InputQueue {
    const fn new() -> Self {
        Self {
            events: [syscall::InputEventWire {
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

    fn push(&mut self, event: syscall::InputEventWire) -> Result<(), i64> {
        if self.len == INPUT_QUEUE_LEN {
            return Err(syscall::ENOBUFS);
        }
        self.events[self.tail] = event;
        self.tail = (self.tail + 1) % INPUT_QUEUE_LEN;
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self) -> Option<syscall::InputEventWire> {
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
static SUBSCRIBER_ENDPOINT: AtomicU64 = AtomicU64::new(0);

#[inline]
fn boot_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_WRITE,
            1,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
        );
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

#[inline]
fn queue_mut() -> &'static mut InputQueue {
    // SAFETY: input_server is a single-threaded event loop; all queue access is
    // serialized in `_start` before a reply is sent.
    unsafe { &mut *QUEUE.0.get() }
}

fn deliver_to_subscriber(event: syscall::InputEventWire, queue_depth: u32) -> bool {
    let endpoint = SUBSCRIBER_ENDPOINT.load(Ordering::Acquire);
    if endpoint == 0 {
        return false;
    }
    let reply = syscall::InputReply {
        status: 0,
        event,
        queue_depth,
        _pad: [0; 4],
    };
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            endpoint,
            &reply as *const syscall::InputReply as u64,
            core::mem::size_of::<syscall::InputReply>() as u64,
            syscall::IPC_FLAG_TIMEOUT,
            0,
            0,
        )
    };
    rc >= 0
}

fn handle(req: &syscall::InputRequest) -> syscall::InputReply {
    match req.msg_type {
        syscall::INPUT_MSG_PUSH => {
            let status = if queue_mut().len == 0 && deliver_to_subscriber(req.event, 0) {
                0
            } else {
                match queue_mut().push(req.event) {
                    Ok(()) => 0,
                    Err(err) => {
                        DROPPED.fetch_add(1, Ordering::Relaxed);
                        err
                    }
                }
            };
            syscall::InputReply {
                status,
                event: syscall::InputEventWire::default(),
                queue_depth: queue_mut().len as u32,
                _pad: [0; 4],
            }
        }
        syscall::INPUT_MSG_POLL => {
            let event = queue_mut().pop();
            syscall::InputReply {
                status: if event.is_some() { 0 } else { syscall::EAGAIN },
                event: event.unwrap_or_default(),
                queue_depth: queue_mut().len as u32,
                _pad: [0; 4],
            }
        }
        syscall::INPUT_MSG_HEARTBEAT => syscall::InputReply {
            status: 0,
            event: syscall::InputEventWire::default(),
            queue_depth: queue_mut().len as u32,
            _pad: [0; 4],
        },
        syscall::INPUT_MSG_ATTACH => {
            SUBSCRIBER_ENDPOINT.store(req.reply_endpoint, Ordering::Release);
            syscall::InputReply {
                status: 0,
                event: syscall::InputEventWire::default(),
                queue_depth: queue_mut().len as u32,
                _pad: [0; 4],
            }
        }
        _ => syscall::InputReply {
            status: syscall::EINVAL,
            event: syscall::InputEventWire::default(),
            queue_depth: queue_mut().len as u32,
            _pad: [0; 4],
        },
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let name = b"input_server";
    boot_log(b"input_server: boot\n");
    let register_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            syscall::INPUT_SERVER_ENDPOINT,
        )
    };
    if register_rc < 0 {
        boot_log(b"input_server: register failed\n");
        exit_failed();
    }
    boot_log(b"input_server: registered\n");
    let mut req = syscall::InputRequest {
        sender_pid: 0,
        msg_type: 0,
        reply_endpoint: 0,
        event: syscall::InputEventWire::default(),
    };
    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                syscall::INPUT_SERVER_ENDPOINT,
                &mut req as *mut syscall::InputRequest as u64,
                core::mem::size_of::<syscall::InputRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 5_000,
            )
        };
        if rc < 0 {
            continue;
        }
        let reply = handle(&req);
        let reply_endpoint = if req.msg_type == syscall::INPUT_MSG_ATTACH {
            0
        } else if req.reply_endpoint != 0 {
            req.reply_endpoint
        } else {
            req.sender_pid as u64
        };
        if reply_endpoint != 0 {
            let _ = unsafe {
                syscall::syscall6(
                    syscall::SYS_IPC_SEND,
                    reply_endpoint,
                    &reply as *const syscall::InputReply as u64,
                    core::mem::size_of::<syscall::InputReply>() as u64,
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
