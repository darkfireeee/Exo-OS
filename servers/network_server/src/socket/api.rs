use exo_syscall_abi as syscall;

pub const SERVER_ENDPOINT_ID: u64 = 7;
pub const IPC_RECV_TIMEOUT_MS: u64 = 5_000;

pub const NETWORK_MSG_HEARTBEAT: u32 = 0;
pub const NETWORK_MSG_SOCKET_OPEN: u32 = 1;
pub const NETWORK_MSG_BIND: u32 = 2;
pub const NETWORK_MSG_CONNECT: u32 = 3;
pub const NETWORK_MSG_SEND: u32 = 4;
pub const NETWORK_MSG_RECV: u32 = 5;
pub const NETWORK_MSG_CLOSE: u32 = 6;
pub const NETWORK_MSG_ROUTE_ADD: u32 = 7;
pub const NETWORK_MSG_ROUTE_QUERY: u32 = 8;
pub const NETWORK_MSG_DRIVER_ATTACH: u32 = 9;
pub const NETWORK_MSG_LINK_SET: u32 = 10;
pub const NETWORK_MSG_ICMP_ECHO: u32 = 11;
pub const NETWORK_MSG_STATS: u32 = 12;
pub const NETWORK_MSG_XDP_ATTACH: u32 = 13;
pub const NETWORK_MSG_BACKEND_SET: u32 = 14;

#[repr(C)]
pub struct NetworkRequest {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub payload: [u8; 120],
}

impl NetworkRequest {
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
pub struct NetworkReply {
    pub status: i64,
    pub handle: u64,
    pub value0: u64,
    pub value1: u64,
    pub flags: u32,
    pub _pad: [u8; 28],
}

impl NetworkReply {
    pub const fn ok(handle: u64, value0: u64, value1: u64, flags: u32) -> Self {
        Self {
            status: 0,
            handle,
            value0,
            value1,
            flags,
            _pad: [0; 28],
        }
    }

    pub const fn error(status: i64) -> Self {
        Self {
            status,
            handle: 0,
            value0: 0,
            value1: 0,
            flags: 0,
            _pad: [0; 28],
        }
    }
}

pub fn register_endpoint() {
    let name = b"network_server";
    // SAFETY: buffer statique valide et endpoint fixe du serveur Ring 1.
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        );
    }
}

pub fn recv_request(request: &mut NetworkRequest) -> Result<bool, i64> {
    // SAFETY: le noyau écrit dans `request`, taille bornée à la struct ABI.
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_RECV,
            request as *mut NetworkRequest as u64,
            core::mem::size_of::<NetworkRequest>() as u64,
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

pub fn send_reply(destination_pid: u32, reply: &NetworkReply) -> i64 {
    // SAFETY: `reply` est une structure POD locale envoyée telle quelle au noyau.
    unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            destination_pid as u64,
            reply as *const NetworkReply as u64,
            core::mem::size_of::<NetworkReply>() as u64,
            0,
            0,
            0,
        )
    }
}

pub fn send_heartbeat() -> NetworkReply {
    // SAFETY: lecture simple du PID courant via syscall sans effet de bord.
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid < 0 {
        NetworkReply::error(pid)
    } else {
        NetworkReply::ok(pid as u64, SERVER_ENDPOINT_ID, 0, 0)
    }
}

#[inline]
pub fn read_u16(payload: &[u8], offset: usize) -> Result<u16, i64> {
    let bytes = payload
        .get(offset..offset.saturating_add(2))
        .ok_or(syscall::EINVAL)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

#[inline]
pub fn read_u32(payload: &[u8], offset: usize) -> Result<u32, i64> {
    let bytes = payload
        .get(offset..offset.saturating_add(4))
        .ok_or(syscall::EINVAL)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[inline]
pub fn read_u64(payload: &[u8], offset: usize) -> Result<u64, i64> {
    let bytes = payload
        .get(offset..offset.saturating_add(8))
        .ok_or(syscall::EINVAL)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}
