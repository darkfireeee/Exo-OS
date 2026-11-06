// ipc/rpc/raw.rs — call_raw: synchronous RPC call from syscall layer
//
// `call_raw` encapsulates a synchronous request/response exchange using
// two raw mailboxes (server + ephemeral responder).
//
// Protocol:
//   1. Allocates an ephemeral reply EndpointId (unique cookie).
//   2. Prefixes message with a `RawCallHeader` header containing this cookie.
//   3. Sends to server mailbox (`server_ep`).
//   4. Awaits return message on the ephemeral mailbox.
//   5. Releases ephemeral mailbox and returns reply payload.
//
// This approach is synchronous and no-alloc. Waiting yields back to the
// scheduler so userspace server can process the request.
//
// RULE IPC-CALL-01: call_raw is ALWAYS synchronous (no callbacks).
// RULE IPC-CALL-02: ephemeral mailboxes are released after each call.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::ipc::channel::raw as channel_raw;
use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::core::types::{EndpointId, IpcError};

const RAW_NOWAIT: u32 = 0x0001;
const CALL_TIMEOUT_NS: u64 = 300_000_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// Cookie counter — unique per call, never reused in session.
// ─────────────────────────────────────────────────────────────────────────────

static CALL_COOKIE: AtomicU64 = AtomicU64::new(1);

#[inline]
fn next_cookie() -> u64 {
    CALL_COOKIE.fetch_add(1, Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// RawCallHeader — header prefixed to each call_raw message
// ─────────────────────────────────────────────────────────────────────────────

/// Magic to validate header.
pub const CALL_MAGIC: u32 = 0x4558_4F43; // "EXOC"

/// Header prefixed to raw call payload.
#[repr(C)]
#[derive(Copy, Clone)]
struct RawCallHeader {
    /// Validation magic.
    magic: u32,
    /// Length of trailing payload (without this header).
    payload_len: u32,
    /// Correlation cookie (unique per call).
    cookie: u64,
    /// Ephemeral reply endpoint.
    reply_ep: u64,
}

const CALL_HEADER_SIZE: usize = core::mem::size_of::<RawCallHeader>();
pub const MAX_CALL_PAYLOAD: usize = MAX_MSG_SIZE - CALL_HEADER_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Ephemeral EndpointId — based on cookie for uniqueness
// ─────────────────────────────────────────────────────────────────────────────

/// Constructs an ephemeral EndpointId from a cookie.
/// Uses private namespace (bit 63 = 1) to avoid collisions
/// with user endpoints.
fn ephemeral_ep(cookie: u64) -> Option<EndpointId> {
    // Bit 63 = 1 -> namespace "kernel ephemeral reply"
    let id = (1u64 << 63) | (cookie & 0x7FFF_FFFF_FFFF_FFFF);
    EndpointId::new(id)
}

#[cfg(not(test))]
#[inline]
fn monotonic_ns() -> u64 {
    crate::scheduler::timer::clock::monotonic_ns()
}

#[cfg(test)]
#[inline]
fn monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[cfg(not(test))]
#[inline]
fn yield_to_peer() {
    unsafe {
        let _ = crate::scheduler::core::switch::cooperative_reschedule();
    }
}

#[cfg(test)]
#[inline]
fn yield_to_peer() {
    std::thread::yield_now();
}

fn send_request(server_ep: EndpointId, data: &[u8], deadline_ns: u64) -> Result<(), IpcError> {
    loop {
        match channel_raw::send_raw(server_ep, data, RAW_NOWAIT) {
            Ok(_) => return Ok(()),
            Err(IpcError::WouldBlock) | Err(IpcError::QueueFull) | Err(IpcError::Full) => {
                if monotonic_ns() >= deadline_ns {
                    return Err(IpcError::Timeout);
                }
                yield_to_peer();
            }
            Err(err) => return Err(err),
        }
    }
}

fn recv_reply(
    reply_ep: EndpointId,
    raw_reply: &mut [u8; MAX_MSG_SIZE],
    deadline_ns: u64,
) -> Result<usize, IpcError> {
    #[cfg(not(test))]
    let current_tcb = crate::scheduler::core::switch::current_thread_raw();
    #[cfg(test)]
    let current_tcb = core::ptr::null_mut();

    loop {
        match channel_raw::recv_raw_or_register_waiter(reply_ep, raw_reply, current_tcb) {
            Ok(Some(n)) => return Ok(n),
            Ok(None) => {
                let now = monotonic_ns();
                if now >= deadline_ns {
                    channel_raw::clear_waiter(reply_ep, current_tcb);
                    return Err(IpcError::Timeout);
                }
                #[cfg(not(test))]
                unsafe {
                    let nap_ns = deadline_ns.saturating_sub(now).min(2_000_000);
                    if !crate::scheduler::timer::sleep_ns(nap_ns) {
                        let _ = crate::scheduler::core::switch::cooperative_reschedule();
                    }
                    channel_raw::clear_waiter(reply_ep, current_tcb);
                }
                #[cfg(test)]
                {
                    std::thread::yield_now();
                    channel_raw::clear_waiter(reply_ep, current_tcb);
                }
            }
            Err(err) => {
                channel_raw::clear_waiter(reply_ep, current_tcb);
                return Err(err);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// call_raw
// ─────────────────────────────────────────────────────────────────────────────

/// Synchronous RPC call to `server_ep`.
///
/// - `msg`      : request payload (<= `MAX_CALL_PAYLOAD` bytes)
/// - `reply_buf`: buffer for reply payload
///
/// Returns number of reply bytes, or an `IpcError`.
///
/// # Errors
/// - `MessageTooLarge`  : `msg.len()` > MAX_CALL_PAYLOAD
/// - `OutOfResources`   : unable to allocate an ephemeral mailbox
/// - `NotFound`         : server not found (mailbox not open)
/// - `Timeout`          : server did not reply within deadline
pub fn call_raw(
    server_ep: EndpointId,
    msg: &[u8],
    reply_buf: &mut [u8],
) -> Result<usize, IpcError> {
    if msg.len() > MAX_CALL_PAYLOAD {
        return Err(IpcError::MessageTooLarge);
    }

    // 1. Allocate cookie and ephemeral reply endpoint.
    let cookie = next_cookie();
    let reply_ep = ephemeral_ep(cookie).ok_or(IpcError::InternalError)?;

    if !channel_raw::mailbox_open(reply_ep) {
        return Err(IpcError::OutOfResources);
    }

    // 2. Build message: header + payload.
    let header = RawCallHeader {
        magic: CALL_MAGIC,
        payload_len: msg.len() as u32,
        cookie,
        reply_ep: reply_ep.get(),
    };

    let mut call_buf = [0u8; MAX_MSG_SIZE];
    // Serialize header (native little-endian, repr(C)).
    // SAFETY: RawCallHeader is repr(C) and call_buf is large enough.
    unsafe {
        core::ptr::write_unaligned(call_buf.as_mut_ptr() as *mut RawCallHeader, header);
    }
    call_buf[CALL_HEADER_SIZE..CALL_HEADER_SIZE + msg.len()].copy_from_slice(msg);

    // 3. Send to server mailbox.
    let deadline_ns = monotonic_ns().saturating_add(CALL_TIMEOUT_NS);
    let send_result = send_request(
        server_ep,
        &call_buf[..CALL_HEADER_SIZE + msg.len()],
        deadline_ns,
    );
    if let Err(e) = send_result {
        channel_raw::mailbox_close(reply_ep);
        return Err(e);
    }

    // 4. Await reply on ephemeral mailbox.
    let mut raw_reply = [0u8; MAX_MSG_SIZE];
    let recv_result = recv_reply(reply_ep, &mut raw_reply, deadline_ns);

    // 5. Release ephemeral mailbox.
    channel_raw::mailbox_close(reply_ep);

    let n = recv_result?;
    if n < CALL_HEADER_SIZE {
        return Err(IpcError::ProtocolError);
    }

    // Verify reply header.
    // SAFETY: n >= CALL_HEADER_SIZE, raw_reply is aligned.
    let reply_hdr: RawCallHeader =
        unsafe { core::ptr::read_unaligned(raw_reply.as_ptr() as *const RawCallHeader) };
    if reply_hdr.magic != CALL_MAGIC || reply_hdr.cookie != cookie {
        return Err(IpcError::ProtocolError);
    }

    let payload_len = reply_hdr.payload_len as usize;
    let copy_len = payload_len.min(reply_buf.len()).min(MAX_CALL_PAYLOAD);
    reply_buf[..copy_len]
        .copy_from_slice(&raw_reply[CALL_HEADER_SIZE..CALL_HEADER_SIZE + copy_len]);
    Ok(copy_len)
}

// ─────────────────────────────────────────────────────────────────────────────
// Server helpers: parse / construct replies
// ─────────────────────────────────────────────────────────────────────────────

/// Result of parsing a message received via a server's mailbox.
pub struct CallRequest<'a> {
    /// Request payload (without header).
    pub payload: &'a [u8],
    /// Correlation cookie (to copy into reply).
    pub cookie: u64,
    /// Reply endpoint (extracted from trailing bytes).
    pub reply_ep: Option<EndpointId>,
}

/// Parses a received raw message (server side).
///
/// Returns `None` if `buf` is not a valid call_raw message.
pub fn parse_call<'a>(buf: &'a [u8]) -> Option<CallRequest<'a>> {
    if buf.len() < CALL_HEADER_SIZE {
        return None;
    }
    // SAFETY: buf.len() >= CALL_HEADER_SIZE.
    let hdr: RawCallHeader =
        unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const RawCallHeader) };
    if hdr.magic != CALL_MAGIC {
        return None;
    }

    let payload_len = hdr.payload_len as usize;
    if buf.len() < CALL_HEADER_SIZE + payload_len {
        return None;
    }

    let reply_ep = EndpointId::new(hdr.reply_ep);

    Some(CallRequest {
        payload: &buf[CALL_HEADER_SIZE..CALL_HEADER_SIZE + payload_len],
        cookie: hdr.cookie,
        reply_ep,
    })
}

/// Constructs and sends a reply towards `reply_ep` (server side).
pub fn send_reply(reply_ep: EndpointId, cookie: u64, reply_data: &[u8]) -> Result<(), IpcError> {
    if reply_data.len() > MAX_CALL_PAYLOAD {
        return Err(IpcError::MessageTooLarge);
    }

    let header = RawCallHeader {
        magic: CALL_MAGIC,
        payload_len: reply_data.len() as u32,
        cookie,
        reply_ep: reply_ep.get(),
    };

    let mut buf = [0u8; MAX_MSG_SIZE];
    // SAFETY: repr(C) + buf large enough.
    unsafe {
        core::ptr::write_unaligned(buf.as_mut_ptr() as *mut RawCallHeader, header);
    }
    buf[CALL_HEADER_SIZE..CALL_HEADER_SIZE + reply_data.len()].copy_from_slice(reply_data);

    channel_raw::send_raw(reply_ep, &buf[..CALL_HEADER_SIZE + reply_data.len()], 0).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_call_preserves_reply_endpoint() {
        let reply_ep = EndpointId::new(0x1234_5678).unwrap();
        let header = RawCallHeader {
            magic: CALL_MAGIC,
            payload_len: 4,
            cookie: 0xAA55,
            reply_ep: reply_ep.get(),
        };
        let mut buf = [0u8; CALL_HEADER_SIZE + 4];
        // SAFETY: `buf` is large enough to hold `repr(C)` header.
        unsafe {
            core::ptr::write_unaligned(buf.as_mut_ptr() as *mut RawCallHeader, header);
        }
        buf[CALL_HEADER_SIZE..CALL_HEADER_SIZE + 4].copy_from_slice(b"ping");

        let req = parse_call(&buf).expect("valid call");
        assert_eq!(req.cookie, 0xAA55);
        assert_eq!(req.payload, b"ping");
        assert_eq!(req.reply_ep, Some(reply_ep));
    }

    #[test]
    fn test_call_raw_roundtrip() {
        let server_ep = EndpointId::new(0x4242).unwrap();
        assert!(channel_raw::mailbox_open(server_ep));

        let worker = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_MSG_SIZE];
            let n = channel_raw::recv_raw(server_ep, &mut buf, 0).expect("recv request");
            let request = parse_call(&buf[..n]).expect("parse request");
            let reply_ep = request.reply_ep.expect("reply endpoint");
            send_reply(reply_ep, request.cookie, request.payload).expect("send reply");
            channel_raw::mailbox_close(server_ep);
        });

        let mut reply = [0u8; 16];
        let n = call_raw(server_ep, b"exo", &mut reply).expect("call succeeds");
        assert_eq!(&reply[..n], b"exo");
        worker.join().expect("worker exits");
    }

    #[test]
    fn test_call_raw_roundtrip_stress() {
        let server_ep = EndpointId::new(0x4343).unwrap();
        assert!(channel_raw::mailbox_open(server_ep));

        let worker = std::thread::spawn(move || {
            let mut iter = 0u32;
            while iter < 128 {
                let mut buf = [0u8; MAX_MSG_SIZE];
                let n = channel_raw::recv_raw(server_ep, &mut buf, 0).expect("recv request");
                let request = parse_call(&buf[..n]).expect("parse request");
                let reply_ep = request.reply_ep.expect("reply endpoint");
                send_reply(reply_ep, request.cookie, request.payload).expect("send reply");
                iter = iter.wrapping_add(1);
            }
            channel_raw::mailbox_close(server_ep);
        });

        let mut iter = 0u32;
        while iter < 128 {
            let mut payload = [0u8; 8];
            payload[..4].copy_from_slice(&iter.to_le_bytes());
            let mut reply = [0u8; 8];
            let n = call_raw(server_ep, &payload[..4], &mut reply).expect("call succeeds");
            assert_eq!(n, 4);
            assert_eq!(&reply[..4], &payload[..4]);
            iter = iter.wrapping_add(1);
        }

        worker.join().expect("worker exits");
    }
}
