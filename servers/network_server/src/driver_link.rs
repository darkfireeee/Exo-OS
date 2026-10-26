use exo_syscall_abi as syscall;

use crate::buf_pool::{NetBufPool, RX_POOL_SIZE, VIRTIO_NET_HDR_SIZE_MODERN};
use crate::protocol::{
    DriverInitMsg, RxReleaseMsg, TxSubmitMsg, NET_CTRL_DRIVER_INIT, NET_CTRL_MAC_QUERY,
    NET_CTRL_RX_RELEASE, NET_CTRL_TX_SUBMIT,
};
use crate::virtio_device::ExoNetDevice;

const DRIVER_RETRY_TICKS: u16 = 32;
const HARDWARE_BOOT_WAIT_MS: u64 = 100;
const HARDWARE_BOOT_POLL_MS: u64 = 10;
const DRIVER_SEND_TIMEOUT_MS: u64 = 10;
const DRIVER_SEND_RETRIES: usize = 32;

pub struct DriverLink {
    endpoint: u64,
    secondary_endpoint: u64,
    ready: bool,
    hardware: bool,
    mac: [u8; 6],
    retry_ticks: u16,
    active_pid: Option<u32>,
}

impl DriverLink {
    pub const fn empty() -> Self {
        Self {
            endpoint: 0,
            secondary_endpoint: 0,
            ready: false,
            hardware: false,
            mac: [0x02, 0x45, 0x58, 0x4f, 0x00, 0x01],
            retry_ticks: 0,
            active_pid: None,
        }
    }

    pub fn connect_net_driver(pool: &NetBufPool) -> Self {
        Self::connect_prefer_hardware(pool, HARDWARE_BOOT_WAIT_MS, true)
    }

    fn connect_retry(pool: &NetBufPool) -> Self {
        Self::connect_prefer_hardware(pool, 0, false)
    }

    fn connect_prefer_hardware(
        pool: &NetBufPool,
        hardware_wait_ms: u64,
        log_timeout: bool,
    ) -> Self {
        let (primary_hw, secondary_hw) = if hardware_wait_ms == 0 {
            lookup_hardware_endpoints()
        } else {
            wait_hardware_endpoints(hardware_wait_ms)
        };
        if let Some(endpoint) = primary_hw {
            let mut link = Self::connect_endpoint(endpoint, pool, true);
            if let Some(sec_ep) = secondary_hw {
                if sec_ep != endpoint {
                    init_single_endpoint(sec_ep, pool);
                    link.secondary_endpoint = sec_ep;
                }
            }
            return link;
        }
        if let Some(endpoint) = lookup_fallback_endpoint() {
            return Self::connect_endpoint(endpoint, pool, false);
        }
        if log_timeout {
            debug_write(b"network_server: hardware wait timeout\n");
            debug_write(b"network_server: no hardware transport\n");
        }
        Self::disconnected_backoff()
    }

    fn connect_endpoint(endpoint: u64, pool: &NetBufPool, hardware: bool) -> Self {
        if hardware {
            debug_write(b"network_server: link hardware\n");
        } else {
            debug_write(b"network_server: link fallback\n");
        }
        let mut link = Self {
            endpoint,
            secondary_endpoint: 0,
            ready: pool.ready(),
            hardware,
            mac: [0x02, 0x45, 0x58, 0x4f, 0x00, 0x01],
            retry_ticks: 0,
            active_pid: None,
        };
        if pool.ready() {
            if !init_single_endpoint(endpoint, pool) {
                link.ready = false;
                link.retry_ticks = DRIVER_RETRY_TICKS;
            }
        }
        link
    }

    fn disconnected_backoff() -> Self {
        let mut link = Self::empty();
        link.retry_ticks = DRIVER_RETRY_TICKS;
        link
    }

    pub const fn ready(&self) -> bool {
        self.ready
    }

    pub const fn hardware_ready(&self) -> bool {
        self.ready && self.hardware
    }

    pub const fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn set_mac(&mut self, mac: [u8; 6]) {
        if mac != [0; 6] {
            self.mac = mac;
        }
    }

    pub fn active_pid(&self) -> Option<u32> {
        self.active_pid
    }

    pub fn set_active_pid(&mut self, pid: u32) {
        if pid != 0 && self.active_pid.is_none() {
            self.active_pid = Some(pid);
        }
    }

    pub fn is_active_sender(&self, _pid: u32) -> bool {
        // Autoriser les deux drivers matériels (virtio_net et e1000) à remonter les trames RX
        true
    }

    pub fn ensure_connected(&mut self, pool: &NetBufPool) {
        if self.retry_ticks != 0 {
            self.retry_ticks -= 1;
            return;
        }
        if !self.hardware {
            let (primary, secondary) = lookup_hardware_endpoints();
            if let Some(endpoint) = primary {
                *self = Self::connect_endpoint(endpoint, pool, true);
                if let Some(sec) = secondary {
                    if sec != endpoint {
                        init_single_endpoint(sec, pool);
                        self.secondary_endpoint = sec;
                    }
                }
                return;
            }
        }
        if self.ready {
            return;
        }
        *self = Self::connect_retry(pool);
    }

    pub fn probe_hardware_now(&mut self, pool: &NetBufPool) -> bool {
        if !self.hardware {
            let (primary, secondary) = lookup_hardware_endpoints();
            if let Some(endpoint) = primary {
                *self = Self::connect_endpoint(endpoint, pool, true);
                if let Some(sec) = secondary {
                    if sec != endpoint {
                        init_single_endpoint(sec, pool);
                        self.secondary_endpoint = sec;
                    }
                }
            } else if !self.ready {
                if let Some(endpoint) = lookup_fallback_endpoint() {
                    *self = Self::connect_endpoint(endpoint, pool, false);
                }
            }
        }
        self.hardware_ready()
    }

    pub fn flush_tx(&self, device: &mut ExoNetDevice, pool: &NetBufPool) {
        while let Some(tx) = device.pop_tx_for_driver() {
            if self.ready {
                let msg = TxSubmitMsg {
                    opcode: NET_CTRL_TX_SUBMIT,
                    pool_idx: tx.pool_idx,
                    len: tx.len,
                };
                let payload = unsafe {
                    core::slice::from_raw_parts(
                        &msg as *const TxSubmitMsg as *const u8,
                        core::mem::size_of::<TxSubmitMsg>(),
                    )
                };
                let rc1 = send(self.endpoint, NET_CTRL_TX_SUBMIT, payload);
                let rc2 = if self.secondary_endpoint != 0 {
                    send(self.secondary_endpoint, NET_CTRL_TX_SUBMIT, payload)
                } else {
                    -1
                };
                if rc1 < 0 && rc2 < 0 {
                    pool.tx_free(tx.pool_idx);
                }
            } else {
                device.dropped_tx = device.dropped_tx.saturating_add(1);
                pool.tx_free(tx.pool_idx);
            }
        }
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
            if self.secondary_endpoint != 0 {
                let _ = send(self.secondary_endpoint, NET_CTRL_RX_RELEASE, payload);
            }
            sent += count;
        }
        device.released_count = 0;
    }
}

fn init_single_endpoint(endpoint: u64, pool: &NetBufPool) -> bool {
    let init = DriverInitMsg {
        opcode: NET_CTRL_DRIVER_INIT,
        pool_count: RX_POOL_SIZE as u32,
        rx_base_iova: pool.rx_base_iova(),
        tx_base_iova: pool.tx_base_iova(),
        hdr_size: VIRTIO_NET_HDR_SIZE_MODERN as u32,
        _pad: 0,
    };
    let payload = unsafe {
        core::slice::from_raw_parts(
            &init as *const DriverInitMsg as *const u8,
            core::mem::size_of::<DriverInitMsg>(),
        )
    };
    let init_rc = send(endpoint, NET_CTRL_DRIVER_INIT, payload);
    if init_rc < 0 {
        debug_errno(b"network_server: driver init errno ", init_rc);
        false
    } else {
        debug_write(b"network_server: driver init sent\n");
        let _ = send(endpoint, NET_CTRL_MAC_QUERY, &[]);
        true
    }
}

fn lookup_hardware_endpoints() -> (Option<u64>, Option<u64>) {
    let virtio = lookup_endpoint(b"virtio_net");
    let e1000 = lookup_endpoint(b"e1000_net");
    match (virtio, e1000) {
        (Some(v), Some(e)) => (Some(v), Some(e)),
        (Some(v), None) => (Some(v), None),
        (None, Some(e)) => (Some(e), None),
        (None, None) => (None, None),
    }
}

fn lookup_fallback_endpoint() -> Option<u64> {
    lookup_endpoint(b"loopback_net").or_else(|| lookup_endpoint(b"loopback_driver"))
}

fn wait_hardware_endpoints(timeout_ms: u64) -> (Option<u64>, Option<u64>) {
    let mut waited = 0u64;
    while waited <= timeout_ms {
        let (p, s) = lookup_hardware_endpoints();
        if p.is_some() {
            return (p, s);
        }
        sleep_ms(HARDWARE_BOOT_POLL_MS);
        waited = waited.saturating_add(HARDWARE_BOOT_POLL_MS);
    }
    lookup_hardware_endpoints()
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

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

fn sleep_ms(ms: u64) {
    let ts = Timespec {
        tv_sec: (ms / 1_000) as i64,
        tv_nsec: ((ms % 1_000) * 1_000_000) as i64,
    };
    let _ = unsafe { syscall::syscall2(syscall::SYS_NANOSLEEP, &ts as *const _ as u64, 0) };
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
    let mut attempts = 0usize;
    loop {
        let rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                endpoint,
                &request as *const DriverRequest as u64,
                core::mem::size_of::<DriverRequest>() as u64,
                syscall::IPC_FLAG_INJECT_SRC_PID
                    | syscall::IPC_FLAG_TIMEOUT
                    | DRIVER_SEND_TIMEOUT_MS,
                0,
                0,
            )
        };
        if rc >= 0 {
            return rc;
        }
        if rc != syscall::EAGAIN && rc != syscall::ETIMEDOUT && rc != syscall::ENOBUFS {
            return rc;
        }
        attempts = attempts.saturating_add(1);
        if attempts >= DRIVER_SEND_RETRIES {
            return rc;
        }
        let _ = unsafe { syscall::syscall0(syscall::SYS_SCHED_YIELD) };
    }
}

fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = byte;
    }
}

fn debug_errno(prefix: &[u8], err: i64) {
    debug_write(prefix);
    let negative = err < 0;
    let mut value = if negative {
        err.wrapping_neg() as u64
    } else {
        err as u64
    };
    if negative {
        debug_write(b"-");
    }
    let mut digits = [0u8; 20];
    let mut pos = digits.len();
    if value == 0 {
        pos -= 1;
        digits[pos] = b'0';
    } else {
        while value != 0 {
            pos -= 1;
            digits[pos] = b'0' + (value % 10) as u8;
            value /= 10;
        }
    }
    debug_write(&digits[pos..]);
    debug_write(b"\n");
}
