use crate::buf_pool::{NetBufPool, PAGE_SIZE};
use crate::virtio_device::{ExoNetDevice, NetBufRef};
use core::cell::RefCell;
use core::sync::atomic::AtomicBool;
use exo_syscall_abi as syscall;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

const ETHERNET_MTU_WITH_HEADER: usize = 1514;
static LOGGED_STACK_RX: AtomicBool = AtomicBool::new(false);
static LOGGED_STACK_TX: AtomicBool = AtomicBool::new(false);
pub(super) static LAST_EGRESS_WAS_ARP: AtomicBool = AtomicBool::new(false);
pub(super) static LAST_EGRESS_SENT_IPV4: AtomicBool = AtomicBool::new(false);

pub(super) struct ExoSmoltcpDevice<'a> {
    pub(super) device: RefCell<&'a mut ExoNetDevice>,
    pub(super) pool: &'a NetBufPool,
}

impl<'a> ExoSmoltcpDevice<'a> {
    pub(super) fn new(device: &'a mut ExoNetDevice, pool: &'a NetBufPool) -> Self {
        Self {
            device: RefCell::new(device),
            pool,
        }
    }

    fn alloc_tx(&self) -> Option<u16> {
        if self.pool.ready() {
            self.pool.tx_alloc()
        } else {
            None
        }
    }
}

impl<'dev> Device for ExoSmoltcpDevice<'dev> {
    type RxToken<'a>
        = ExoRxToken<'a, 'dev>
    where
        Self: 'a;
    type TxToken<'a>
        = ExoTxToken<'a, 'dev>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.pool.ready() {
            return None;
        }

        let rx = self.device.borrow_mut().pop_rx_for_stack()?;
        let tx_idx = self.alloc_tx();

        Some((
            ExoRxToken {
                device: &self.device,
                pool: self.pool,
                rx,
            },
            ExoTxToken {
                device: &self.device,
                pool: self.pool,
                pool_idx: tx_idx,
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let pool_idx = self.alloc_tx()?;
        Some(ExoTxToken {
            device: &self.device,
            pool: self.pool,
            pool_idx: Some(pool_idx),
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit =
            ETHERNET_MTU_WITH_HEADER.min(PAGE_SIZE.saturating_sub(self.pool.hdr_size()));
        caps.max_burst_size = Some(16);
        caps
    }
}

pub(super) struct ExoRxToken<'a, 'dev> {
    pub(super) device: &'a RefCell<&'dev mut ExoNetDevice>,
    pub(super) pool: &'dev NetBufPool,
    rx: NetBufRef,
}

impl RxToken for ExoRxToken<'_, '_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let len = (self.rx.len as usize).min(PAGE_SIZE.saturating_sub(self.pool.hdr_size()));
        let payload = unsafe {
            core::slice::from_raw_parts(
                self.pool.rx_payload_ptr_mut(self.rx.pool_idx as usize),
                len,
            )
        };
        log_first_frame(
            &LOGGED_STACK_RX,
            b"network_server: first stack rx ",
            payload,
        );
        let result = f(payload);
        self.device.borrow_mut().release_rx(self.rx.pool_idx);
        result
    }
}

pub(super) struct ExoTxToken<'a, 'dev> {
    pub(super) device: &'a RefCell<&'dev mut ExoNetDevice>,
    pub(super) pool: &'dev NetBufPool,
    pool_idx: Option<u16>,
}

impl TxToken for ExoTxToken<'_, '_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let max_len = PAGE_SIZE.saturating_sub(self.pool.hdr_size());
        let len = len.min(max_len);
        if let Some(pool_idx) = self.pool_idx {
            unsafe {
                core::ptr::write_bytes(
                    self.pool.tx_header_ptr_mut(pool_idx as usize),
                    0,
                    self.pool.hdr_size(),
                );
            }
            let payload = unsafe {
                core::slice::from_raw_parts_mut(
                    self.pool.tx_payload_ptr_mut(pool_idx as usize),
                    len,
                )
            };
            let result = f(payload);
            if payload.len() >= 14 {
                let ethertype = u16::from_be_bytes([payload[12], payload[13]]);
                if ethertype == 0x0806 {
                    LAST_EGRESS_WAS_ARP.store(true, core::sync::atomic::Ordering::Relaxed);
                } else if ethertype == 0x0800 {
                    LAST_EGRESS_SENT_IPV4.store(true, core::sync::atomic::Ordering::Relaxed);
                }
            }
            log_first_frame(
                &LOGGED_STACK_TX,
                b"network_server: first stack tx ",
                payload,
            );
            let queued = self.device.borrow_mut().queue_tx_idx(pool_idx, len);
            if queued.is_err() {
                self.pool.tx_free(pool_idx);
            }
            return result;
        }

        let mut drop_buf = [0u8; ETHERNET_MTU_WITH_HEADER];
        let scratch_len = len.min(drop_buf.len());
        let result = f(&mut drop_buf[..scratch_len]);
        let mut device = self.device.borrow_mut();
        device.dropped_rx_tx_token = device.dropped_rx_tx_token.saturating_add(1);
        result
    }
}

fn log_first_frame(flag: &AtomicBool, prefix: &[u8], frame: &[u8]) {
    let _ = (flag, prefix, frame);
}

#[allow(dead_code)]
pub(super) fn debug_write(bytes: &[u8]) {
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
