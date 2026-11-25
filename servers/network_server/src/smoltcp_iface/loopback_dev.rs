use core::cell::UnsafeCell;
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

const LOOPBACK_MTU: usize = 2048;
const LOOPBACK_RING_LEN: usize = 64;

pub(super) struct LoopbackQueue {
    frames: [[u8; LOOPBACK_MTU]; LOOPBACK_RING_LEN],
    lens: [usize; LOOPBACK_RING_LEN],
    head: usize,
    tail: usize,
    count: usize,
}

impl LoopbackQueue {
    const fn new() -> Self {
        Self {
            frames: [[0u8; LOOPBACK_MTU]; LOOPBACK_RING_LEN],
            lens: [0; LOOPBACK_RING_LEN],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push_frame(&mut self, len: usize) -> Option<&mut [u8]> {
        if self.count >= LOOPBACK_RING_LEN || len > LOOPBACK_MTU {
            return None;
        }
        let idx = self.tail & (LOOPBACK_RING_LEN - 1);
        self.lens[idx] = len;
        self.tail = self.tail.wrapping_add(1);
        self.count += 1;
        Some(&mut self.frames[idx][..len])
    }

    fn pop_frame(&mut self) -> Option<([u8; LOOPBACK_MTU], usize)> {
        if self.count == 0 {
            return None;
        }
        let idx = self.head & (LOOPBACK_RING_LEN - 1);
        let len = self.lens[idx];
        let mut copy = [0u8; LOOPBACK_MTU];
        copy[..len].copy_from_slice(&self.frames[idx][..len]);
        self.head = self.head.wrapping_add(1);
        self.count -= 1;
        Some((copy, len))
    }

    pub(super) const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

pub(super) struct LoopbackStorage {
    pub(super) queue: UnsafeCell<LoopbackQueue>,
}

// SAFETY: uniquement emprunté sous NETWORK_SERVICE.
unsafe impl Sync for LoopbackStorage {}

pub(super) static LOOPBACK_QUEUE: LoopbackStorage = LoopbackStorage {
    queue: UnsafeCell::new(LoopbackQueue::new()),
};

pub(super) struct LoopbackIpDevice;

pub(super) struct LoopbackRxToken {
    buf: [u8; LOOPBACK_MTU],
    len: usize,
}

pub(super) struct LoopbackTxToken;

impl RxToken for LoopbackRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buf[..self.len])
    }
}

impl TxToken for LoopbackTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // SAFETY: accès sérialisé par NETWORK_SERVICE.
        let q = unsafe { &mut *LOOPBACK_QUEUE.queue.get() };
        if let Some(slot) = q.push_frame(len) {
            f(slot)
        } else {
            let mut drop_buf = [0u8; LOOPBACK_MTU];
            let clamped = len.min(drop_buf.len());
            f(&mut drop_buf[..clamped])
        }
    }
}

impl Device for LoopbackIpDevice {
    type RxToken<'a> = LoopbackRxToken;
    type TxToken<'a> = LoopbackTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let q = unsafe { &mut *LOOPBACK_QUEUE.queue.get() };
        let (buf, len) = q.pop_frame()?;
        Some((LoopbackRxToken { buf, len }, LoopbackTxToken))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        let q = unsafe { &*LOOPBACK_QUEUE.queue.get() };
        if q.count < LOOPBACK_RING_LEN {
            Some(LoopbackTxToken)
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = LOOPBACK_MTU;
        caps.max_burst_size = Some(16);
        caps
    }
}

