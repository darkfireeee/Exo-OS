use exo_syscall_abi as syscall;

use crate::buf_pool::{NetBufPool, RX_POOL_SIZE, VIRTIO_NET_HDR_SIZE_LEGACY};
use crate::protocol::{
    DriverInitMsg, RxReleaseMsg, NET_CTRL_DRIVER_INIT, NET_CTRL_MAC_QUERY, NET_CTRL_RX_RELEASE,
};
use crate::virtio_device::ExoNetDevice;

pub struct DriverLink {
    endpoint: u64,
    ready: bool,
    mac: [u8; 6],
}

impl DriverLink {
    pub const fn empty() -> Self {
        Self {
            endpoint: 0,
            ready: false,
            mac: [0x02, 0x45, 0x58, 0x4f, 0x00, 0x01],
        }
    }

    pub fn connect_virtio_net(pool: &NetBufPool) -> Self {
        let endpoint =
            lookup_endpoint(b"virtio_net").or_else(|| lookup_endpoint(b"virtio_drivers"));
        let Some(endpoint) = endpoint else {
            return Self::empty();
        };

        let mut link = Self {
            endpoint,
            ready: pool.ready(),
            mac: [0x02, 0x45, 0x58, 0x4f, 0x00, 0x01],
        };
        if pool.ready() {
            let init = DriverInitMsg {
                opcode: NET_CTRL_DRIVER_INIT,
                pool_count: RX_POOL_SIZE as u32,
                rx_base_iova: pool.rx_base_iova(),
                tx_base_iova: pool.tx_base_iova(),
                hdr_size: VIRTIO_NET_HDR_SIZE_LEGACY as u32,
                _pad: 0,
            };
            let payload = unsafe {
                core::slice::from_raw_parts(
                    &init as *const DriverInitMsg as *const u8,
                    core::mem::size_of::<DriverInitMsg>(),
                )
            };
            if send(endpoint, NET_CTRL_DRIVER_INIT, payload) < 0 {
                link.ready = false;
            }
            let _ = send(endpoint, NET_CTRL_MAC_QUERY, &[]);
        }
        link
    }

    pub const fn ready(&self) -> bool {
        self.ready
    }

    pub const fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn flush_released(&self, device: &mut ExoNetDevice) {
        if !self.ready || device.released_count == 0 {
            device.released_count = 0;
            return;
        }

        let mut sent = 0usize;
        while sent < device.released_count {
            let count = (device.released_count - sent).min(20);
            let mut msg = RxReleaseMsg {
                opcode: NET_CTRL_RX_RELEASE,
                count: count as u32,
                pool_idx: [0; 20],
            };
            msg.pool_idx[..count].copy_from_slice(&device.released_buf[sent..sent + count]);
            let payload = unsafe {
                core::slice::from_raw_parts(
                    &msg as *const RxReleaseMsg as *const u8,
                    core::mem::size_of::<RxReleaseMsg>(),
                )
            };
            let _ = send(self.endpoint, NET_CTRL_RX_RELEASE, payload);
            sent += count;
        }
        device.released_count = 0;
    }
}

fn lookup_endpoint(name: &[u8]) -> Option<u64> {
    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_IPC_LOOKUP,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    if rc > 0 {
        Some(rc as u64)
    } else {
        None
    }
}

fn send(endpoint: u64, msg_type: u32, payload: &[u8]) -> i64 {
    #[repr(C)]
    struct DriverRequest {
        sender_pid: u32,
        msg_type: u32,
        payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
    }

    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) }.max(0) as u32;
    let mut request = DriverRequest {
        sender_pid: pid,
        msg_type,
        payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    let n = payload.len().min(request.payload.len());
    request.payload[..n].copy_from_slice(&payload[..n]);
    unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            endpoint,
            &request as *const DriverRequest as u64,
            core::mem::size_of::<DriverRequest>() as u64,
            0,
            0,
            0,
        )
    }
}
