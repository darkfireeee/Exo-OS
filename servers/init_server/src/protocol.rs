use super::syscall;

pub const SERVER_ENDPOINT_ID: u64 = 1;
pub const IPC_RECV_TIMEOUT_MS: u64 = 10;
pub const SERVICE_NAME_MAX: usize = 64;

pub const INIT_MSG_HEARTBEAT: u32 = 0;
pub const INIT_MSG_START: u32 = 1;
pub const INIT_MSG_STOP: u32 = 2;
pub const INIT_MSG_STATUS: u32 = 3;
pub const INIT_MSG_RESTART: u32 = 4;
pub const INIT_MSG_CHILD_DIED: u32 = 5;
pub const INIT_MSG_PREPARE_ISOLATION: u32 = 6;
pub const INIT_MSG_PREPARE_ISOLATION_ACK: u32 = 7;

#[repr(C)]
pub struct InitRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub payload: [u8; 120],
}

impl InitRequest {
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; 120],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InitReply {
    pub status: i64,
    pub data: [u8; 56],
}

impl InitReply {
    pub const fn ok() -> Self {
        Self {
            status: 0,
            data: [0; 56],
        }
    }

    pub const fn error(status: i64) -> Self {
        Self {
            status,
            data: [0; 56],
        }
    }
}

pub fn register_endpoint() {
    let name = b"init_server";
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        );
    }
}

pub fn recv_request(request: &mut InitRequest) -> Result<bool, i64> {
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_RECV,
            request as *mut InitRequest as u64,
            core::mem::size_of::<InitRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
        )
    };

    if rc == syscall::ETIMEDOUT {
        return Ok(false);
    }
    if rc < 0 {
        return Err(rc);
    }
    Ok(true)
}

pub fn send_reply(destination_pid: u32, reply: &InitReply) -> i64 {
    unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            destination_pid as u64,
            reply as *const InitReply as u64,
            core::mem::size_of::<InitReply>() as u64,
            0,
            0,
            0,
        )
    }
}

#[inline]
pub fn read_service_name(payload: &[u8]) -> Option<&[u8]> {
    let len = payload.first().copied()? as usize;
    if len == 0 || len > SERVICE_NAME_MAX {
        return None;
    }
    let end = 1usize.checked_add(len)?;
    payload.get(1..end)
}

#[inline]
pub fn read_u32(payload: &[u8], offset: usize) -> Result<u32, i64> {
    let bytes = payload
        .get(offset..offset.saturating_add(4))
        .ok_or(syscall::EINVAL)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[inline]
pub fn read_i32(payload: &[u8], offset: usize) -> Result<i32, i64> {
    let bytes = payload
        .get(offset..offset.saturating_add(4))
        .ok_or(syscall::EINVAL)?;
    Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub fn heartbeat_reply(pid: u32, running_count: u32, running_mask: u64) -> InitReply {
    let mut reply = InitReply::ok();
    reply.data[0..4].copy_from_slice(&pid.to_le_bytes());
    reply.data[4..8].copy_from_slice(&running_count.to_le_bytes());
    reply.data[8..16].copy_from_slice(&running_mask.to_le_bytes());
    reply
}

pub fn status_reply(
    index: u32,
    pid: u32,
    delay_ticks: u32,
    running: bool,
    critical: bool,
) -> InitReply {
    let mut reply = InitReply::ok();
    reply.data[0..4].copy_from_slice(&index.to_le_bytes());
    reply.data[4..8].copy_from_slice(&pid.to_le_bytes());
    reply.data[8..12].copy_from_slice(&delay_ticks.to_le_bytes());
    reply.data[12] = if running { 1 } else { 0 };
    reply.data[13] = if critical { 1 } else { 0 };
    reply
}

pub fn lifecycle_reply(pid: u32, running_mask: u64) -> InitReply {
    let mut reply = InitReply::ok();
    reply.data[0..4].copy_from_slice(&pid.to_le_bytes());
    reply.data[8..16].copy_from_slice(&running_mask.to_le_bytes());
    reply
}

pub fn isolation_reply(
    checkpoint_tag: &[u8; 32],
    running_count: u32,
    running_mask: u64,
) -> InitReply {
    let mut reply = InitReply::ok();
    reply.data[0..32].copy_from_slice(checkpoint_tag);
    reply.data[32..36].copy_from_slice(&running_count.to_le_bytes());
    reply.data[36..44].copy_from_slice(&running_mask.to_le_bytes());
    reply.data[44..48].copy_from_slice(&INIT_MSG_PREPARE_ISOLATION_ACK.to_le_bytes());
    reply
}
