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

// FIX-INPUT-MULTI (ANALYSE_SERVERS_EXOOS §R4) : l'ancienne implémentation
// n'utilisait qu'un seul AtomicU64 SUBSCRIBER_ENDPOINT. Si tty_server et
// exosh s'attachaient tous les deux, le second écrasait le premier silencieusement.
// Multi-applications clavier (ex: futur Wayland) impossible.
//
// Correction : table statique de MAX_SUBSCRIBERS abonnés.
// Chaque slot est un AtomicU64 (0 = slot vide).
// INPUT_MSG_ATTACH enregistre dans le premier slot libre.
// deliver_to_subscribers diffuse à tous les slots actifs.
const MAX_SUBSCRIBERS: usize = 4;
struct SubscriberTable {
    endpoints: [AtomicU64; MAX_SUBSCRIBERS],
}
impl SubscriberTable {
    const fn new() -> Self {
        Self {
            endpoints: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }
    fn attach(&self, endpoint: u64) -> bool {
        for slot in &self.endpoints {
            if slot.load(Ordering::Acquire) == 0 {
                // CAS : si encore vide, on prend le slot
                if slot.compare_exchange(0, endpoint, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    return true;
                }
            }
            // Si ce slot a déjà cet endpoint enregistré, pas de doublon
            if slot.load(Ordering::Acquire) == endpoint {
                return true;
            }
        }
        false // table pleine
    }
    fn detach(&self, endpoint: u64) {
        for slot in &self.endpoints {
            let _ = slot.compare_exchange(endpoint, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
    fn deliver_all(&self, event: syscall::InputEventWire, queue_depth: u32) -> bool {
        let mut delivered = false;
        let reply = syscall::InputReply {
            status: 0,
            event,
            queue_depth,
            _pad: [0; 4],
        };
        for slot in &self.endpoints {
            let ep = slot.load(Ordering::Acquire);
            if ep == 0 {
                continue;
            }
            let rc = unsafe {
                syscall::syscall6(
                    syscall::SYS_IPC_SEND,
                    ep,
                    &reply as *const syscall::InputReply as u64,
                    core::mem::size_of::<syscall::InputReply>() as u64,
                    0, 0, 0,
                )
            };
            if rc >= 0 {
                delivered = true;
            } else {
                // Endpoint mort → retirer le slot pour libérer la place
                let _ = slot.compare_exchange(ep, 0, Ordering::AcqRel, Ordering::Acquire);
            }
        }
        delivered
    }
}
static SUBSCRIBERS: SubscriberTable = SubscriberTable::new();

#[inline]
fn boot_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            1,
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

fn deliver_to_subscriber(event: syscall::InputEventWire, queue_depth: u32) -> bool {
    // FIX-INPUT-MULTI: diffusion à tous les abonnés enregistrés.
    SUBSCRIBERS.deliver_all(event, queue_depth)
}

#[inline]
fn queue_mut() -> &'static mut InputQueue {
    // SAFETY: input_server is a single-threaded event loop; all queue access is
    // serialized in `_start` before a reply is sent.
    unsafe { &mut *QUEUE.0.get() }
}

fn handle(req: &syscall::InputRequest) -> syscall::InputReply {
    match req.msg_type {
        syscall::INPUT_MSG_PUSH => {
            let status = if deliver_to_subscriber(req.event, queue_mut().len as u32) {
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
            // FIX-INPUT-MULTI: enregistrement dans la table multi-abonnés.
            let ok = SUBSCRIBERS.attach(req.reply_endpoint);
            syscall::InputReply {
                status: if ok { 0 } else { syscall::ENOMEM },
                event: syscall::InputEventWire::default(),
                queue_depth: queue_mut().len as u32,
                _pad: [0; 4],
            }
        }
        syscall::INPUT_MSG_DETACH => {
            // FIX-INPUT-MULTI: support du détachement (nouveau type de message).
            SUBSCRIBERS.detach(req.reply_endpoint);
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
