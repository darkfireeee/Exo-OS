#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_tty::{LineDiscipline, LineEvent, Signal};

const LINE_OUT_MAX: usize = syscall::TTY_LINE_MAX;
const TTY_INPUT_STREAM_CHANNEL: u64 = 0x5454_5902;
const TTY_RECV_TIMEOUT_MS: u64 = 25;
const INPUT_DRAIN_LIMIT: usize = 32;
const INPUT_DRAIN_TIMEOUT_MS: u64 = 1;
const FB_SEND_RETRY_LIMIT: usize = 8;
const RAW_CALL_MAGIC: u32 = 0x4558_4F43;

#[repr(C)]
#[derive(Clone, Copy)]
struct RawCallHeader {
    magic: u32,
    payload_len: u32,
    cookie: u64,
    reply_ep: u64,
}

const RAW_CALL_HEADER_SIZE: usize = core::mem::size_of::<RawCallHeader>();

#[inline]
fn boot_log(bytes: &[u8]) {
    let _ = fb_write_all(bytes);
}

#[inline]
fn debug_log(bytes: &[u8]) {
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

fn console_write(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let _ = fb_write_all(bytes);
}

fn fb_endpoint_ready() -> bool {
    let name = b"fb_server";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_LOOKUP,
            name.as_ptr() as u64,
            name.len() as u64,
            0,
        )
    };
    rc == syscall::FB_SERVER_ENDPOINT as i64
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

fn current_pid() -> Option<u64> {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    (pid > 0).then_some(pid as u64)
}

#[derive(Clone, Copy)]
struct InputEndpoints {
    stream: u64,
}

fn register_input_endpoint(pid: u64, channel: u64, name: &[u8]) -> Option<u64> {
    let endpoint = (pid << 32) | channel;
    let register_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
    if register_rc < 0 && register_rc != syscall::EEXIST {
        return None;
    }
    Some(endpoint)
}

fn attach_input_stream(stream_endpoint: u64) -> bool {
    let req = syscall::InputRequest {
        sender_pid: 0,
        msg_type: syscall::INPUT_MSG_ATTACH,
        reply_endpoint: stream_endpoint,
        event: syscall::InputEventWire::default(),
    };
    let attach_rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            0,
            0,
            0,
        )
    };
    attach_rc >= 0
}

fn register_input_endpoints() -> Option<InputEndpoints> {
    let pid = current_pid()?;
    let stream = register_input_endpoint(pid, TTY_INPUT_STREAM_CHANNEL, b"tty_input_stream")?;
    attach_input_stream(stream).then_some(InputEndpoints { stream })
}

fn fb_write_all(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    if !fb_endpoint_ready() {
        debug_log(bytes);
        return false;
    }
    let mut done = 0usize;
    while done < bytes.len() {
        let n = core::cmp::min(bytes.len() - done, syscall::FB_TEXT_MAX);
        let mut req = syscall::FbRequest::zeroed();
        req.msg_type = syscall::FB_MSG_WRITE;
        req.a = n as u64;
        req.reply_endpoint = 0;
        req.data[..n].copy_from_slice(&bytes[done..done + n]);
        let mut retries = 0usize;
        loop {
            let rc = unsafe {
                syscall::syscall6(
                    syscall::SYS_IPC_SEND,
                    syscall::FB_SERVER_ENDPOINT,
                    &req as *const syscall::FbRequest as u64,
                    core::mem::size_of::<syscall::FbRequest>() as u64,
                    syscall::IPC_FLAG_TIMEOUT,
                    0,
                    0,
                )
            };
            if rc >= 0 {
                break;
            }
            if (rc != syscall::EAGAIN && rc != syscall::ETIMEDOUT) || retries >= FB_SEND_RETRY_LIMIT
            {
                debug_log(bytes);
                return false;
            }
            retries += 1;
            unsafe {
                let _ = syscall::syscall0(syscall::SYS_SCHED_YIELD);
            }
        }
        done += n;
    }
    debug_log(bytes);
    true
}

struct TtyState {
    line: LineDiscipline,
    ready: [u8; LINE_OUT_MAX],
    ready_len: usize,
    // FIX-SRV-M4 (ANALYSE_SERVERS §M4) : queue de 4 lectures en attente
    // (au lieu de 1) pour gérer les appels concurrent de exosh + scripts.
    // Si la queue est pleine, EAGAIN est retourné immédiatement.
    pending_reads: [Option<RawCallHeader>; 4],
    pending_reads_count: usize,
}

impl TtyState {
    const fn new() -> Self {
        Self {
            line: LineDiscipline::new(),
            ready: [0; LINE_OUT_MAX],
            ready_len: 0,
            pending_reads: [None, None, None, None],
            pending_reads_count: 0,
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
            complete_pending_read();
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
        Some(LineEvent::ClearScreen) => {
            console_write(b"\x0c");
            reply(0, 0, b"\x0c")
        }
        None => reply(0, 0, &[]),
    }
}

fn handle_input_event(event: syscall::InputEventWire) {
    if event.device != syscall::INPUT_DEVICE_KEYBOARD
        || event.state != syscall::INPUT_KEY_PRESSED
        || event.ascii == 0
    {
        return;
    }
    let _ = handle_input(event.ascii);
}

fn recv_input_reply(endpoint: u64, timeout_ms: u64) -> Option<syscall::InputReply> {
    let mut reply = syscall::InputReply::default();
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            endpoint,
            &mut reply as *mut syscall::InputReply as u64,
            core::mem::size_of::<syscall::InputReply>() as u64,
            syscall::IPC_FLAG_TIMEOUT | timeout_ms,
        )
    };
    (rc >= 0).then_some(reply)
}

fn drain_stream_input_events(endpoint: u64) {
    let mut drained = 0usize;
    while drained < INPUT_DRAIN_LIMIT {
        let Some(reply) = recv_input_reply(endpoint, INPUT_DRAIN_TIMEOUT_MS) else {
            break;
        };
        if reply.status == 0 {
            handle_input_event(reply.event);
        }
        drained += 1;
    }
}

fn drain_input_events(input_endpoints: Option<InputEndpoints>) {
    let Some(endpoints) = input_endpoints else {
        return;
    };
    drain_stream_input_events(endpoints.stream);
}

fn handle_read_line() -> syscall::TtyReply {
    take_ready_line().unwrap_or_else(|| reply(syscall::EAGAIN, 0, &[]))
}

fn take_ready_line() -> Option<syscall::TtyReply> {
    let state = tty_mut();
    let n = state.ready_len;
    if n == 0 {
        return None;
    }
    let mut data = [0u8; LINE_OUT_MAX];
    data[..n].copy_from_slice(&state.ready[..n]);
    state.ready_len = 0;
    Some(syscall::TtyReply {
        status: 0,
        signal: 0,
        len: n as u32,
        data,
    })
}

fn complete_pending_read() {
    // FIX-SRV-M4 : dépiler le premier reader en attente.
    let state = tty_mut();
    let header = if state.pending_reads_count == 0 {
        return;
    } else {
        let h = state.pending_reads[0].take().unwrap();
        // Shift down
        for i in 0..3 { state.pending_reads[i] = state.pending_reads[i+1].take(); }
        state.pending_reads_count -= 1;
        h
    };
    let Some(_dummy) = Some(header) else {
        return;
    };
    let Some(response) = take_ready_line() else {
        // FIX: pending_read renommé en pending_reads (queue de 4 slots)
        let state = tty_mut();
        if state.pending_reads_count < 4 {
            state.pending_reads[state.pending_reads_count] = Some(header);
            state.pending_reads_count += 1;
        }
        return;
    };
    send_raw_call_reply(header.reply_ep, header.cookie, &response);
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

fn read_tty_request(bytes: &[u8]) -> Option<syscall::TtyRequest> {
    if bytes.len() < core::mem::size_of::<syscall::TtyRequest>() {
        return None;
    }
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const syscall::TtyRequest) })
}

fn parse_raw_call(bytes: &[u8]) -> Option<(RawCallHeader, &[u8])> {
    if bytes.len() < RAW_CALL_HEADER_SIZE {
        return None;
    }
    let header = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const RawCallHeader) };
    if header.magic != RAW_CALL_MAGIC {
        return None;
    }
    let payload_len = header.payload_len as usize;
    if bytes.len() < RAW_CALL_HEADER_SIZE.saturating_add(payload_len) {
        return None;
    }
    Some((
        header,
        &bytes[RAW_CALL_HEADER_SIZE..RAW_CALL_HEADER_SIZE + payload_len],
    ))
}

fn send_raw_call_reply(reply_ep: u64, cookie: u64, reply: &syscall::TtyReply) {
    if reply_ep == 0 {
        return;
    }
    let reply_bytes = unsafe {
        core::slice::from_raw_parts(
            reply as *const syscall::TtyReply as *const u8,
            core::mem::size_of::<syscall::TtyReply>(),
        )
    };
    let total = RAW_CALL_HEADER_SIZE.saturating_add(reply_bytes.len());
    if total > syscall::IPC_KERNEL_MAX_MSG_SIZE {
        return;
    }

    let mut out = [0u8; syscall::IPC_KERNEL_MAX_MSG_SIZE];
    let header = RawCallHeader {
        magic: RAW_CALL_MAGIC,
        payload_len: reply_bytes.len() as u32,
        cookie,
        reply_ep,
    };
    unsafe {
        core::ptr::write_unaligned(out.as_mut_ptr() as *mut RawCallHeader, header);
    }
    out[RAW_CALL_HEADER_SIZE..total].copy_from_slice(reply_bytes);
    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            reply_ep,
            out.as_ptr() as u64,
            total as u64,
            0,
            0,
            0,
        )
    };
}

fn send_tty_reply(reply_endpoint: u64, response: &syscall::TtyReply) {
    if reply_endpoint == 0 {
        return;
    }
    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            reply_endpoint,
            response as *const syscall::TtyReply as u64,
            core::mem::size_of::<syscall::TtyReply>() as u64,
            0,
            0,
            0,
        )
    };
}

fn dispatch_message(bytes: &[u8]) {
    if let Some((header, payload)) = parse_raw_call(bytes) {
        let response = match read_tty_request(payload) {
            Some(req) if req.msg_type == syscall::TTY_MSG_READ_LINE => {
                if let Some(response) = take_ready_line() {
                    response
                } else {
                    let state = tty_mut();
                    if state.pending_reads_count < 4 {
                        // FIX-SRV-M4 : enqueue dans la queue circulaire
                        state.pending_reads[state.pending_reads_count] = Some(header);
                        state.pending_reads_count += 1;
                        return;
                    }
                    reply(syscall::EAGAIN, 0, &[])
                }
            }
            Some(req) => handle(&req),
            None => reply(syscall::EINVAL, 0, &[]),
        };
        send_raw_call_reply(header.reply_ep, header.cookie, &response);
        return;
    }

    let Some(req) = read_tty_request(bytes) else {
        return;
    };
    let response = handle(&req);
    let reply_endpoint = if req.reply_endpoint != 0 {
        req.reply_endpoint
    } else {
        req.sender_pid as u64
    };
    send_tty_reply(reply_endpoint, &response);
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
    let mut input_endpoints = register_input_endpoints();
    if input_endpoints.is_some() {
        boot_log(b"tty_server: input attached\n");
    } else {
        boot_log(b"tty_server: input attach pending\n");
    }
    let mut recv_buf = [0u8; syscall::IPC_KERNEL_MAX_MSG_SIZE];
    loop {
        if input_endpoints.is_none() {
            input_endpoints = register_input_endpoints();
        }
        drain_input_events(input_endpoints);
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                syscall::TTY_SERVER_ENDPOINT,
                recv_buf.as_mut_ptr() as u64,
                recv_buf.len() as u64,
                syscall::IPC_FLAG_TIMEOUT | TTY_RECV_TIMEOUT_MS,
            )
        };
        if rc < 0 {
            continue;
        }
        dispatch_message(&recv_buf[..rc as usize]);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
