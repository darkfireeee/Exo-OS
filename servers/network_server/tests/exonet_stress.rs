use std::collections::{BTreeSet, VecDeque};

const POOL_SIZE: usize = 32;
const RING_SIZE: usize = 16;
const MAX_SOCKETS: usize = 64;
const BATCH_SIZE: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BootPhase {
    Uninit,
    DriverInitSent,
    Ready,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PhoenixPhase {
    Normal,
    Draining,
    Serialized,
}

struct Model {
    rx_submitted: [bool; POOL_SIZE],
    tx_inflight: [bool; POOL_SIZE],
    ipc_rx_ring: VecDeque<usize>,
    ipc_tx_ring: VecDeque<Vec<usize>>,
    ipc_pkt_ring: VecDeque<usize>,
    ns_released_buf: BTreeSet<usize>,
    tx_pool_free: BTreeSet<usize>,
    socket_slots: BTreeSet<usize>,
    phoenix_phase: PhoenixPhase,
    boot_phase: BootPhase,
    dropped_rx: u8,
    dropped_tx: u8,
}

impl Model {
    fn new() -> Self {
        Self {
            rx_submitted: [true; POOL_SIZE],
            tx_inflight: [false; POOL_SIZE],
            ipc_rx_ring: VecDeque::new(),
            ipc_tx_ring: VecDeque::new(),
            ipc_pkt_ring: VecDeque::new(),
            ns_released_buf: BTreeSet::new(),
            tx_pool_free: (0..POOL_SIZE).collect(),
            socket_slots: BTreeSet::new(),
            phoenix_phase: PhoenixPhase::Normal,
            boot_phase: BootPhase::Uninit,
            dropped_rx: 0,
            dropped_tx: 0,
        }
    }

    fn send_driver_init(&mut self) {
        if self.boot_phase == BootPhase::Uninit && self.phoenix_phase == PhoenixPhase::Normal {
            self.boot_phase = BootPhase::DriverInitSent;
        }
    }

    fn ack_driver_init(&mut self) {
        if self.boot_phase == BootPhase::DriverInitSent {
            self.boot_phase = BootPhase::Ready;
        }
    }

    fn handle_irq(&mut self, idx: usize) {
        if self.boot_phase != BootPhase::Ready || self.phoenix_phase != PhoenixPhase::Normal {
            return;
        }
        if !self.rx_submitted[idx] {
            return;
        }
        if self.ipc_rx_ring.len() < RING_SIZE {
            self.rx_submitted[idx] = false;
            self.ipc_rx_ring.push_back(idx);
        } else {
            self.dropped_rx = self.dropped_rx.saturating_add(1).min(3);
        }
    }

    fn poll_one(&mut self) {
        if self.boot_phase == BootPhase::Ready && self.phoenix_phase == PhoenixPhase::Normal {
            if let Some(idx) = self.ipc_rx_ring.pop_front() {
                self.ns_released_buf.insert(idx);
            }
        }
    }

    fn flush_released(&mut self) {
        if self.boot_phase != BootPhase::Ready || self.ns_released_buf.is_empty() {
            return;
        }
        if self.ipc_tx_ring.len() >= RING_SIZE {
            return;
        }
        let batch: Vec<_> = self
            .ns_released_buf
            .iter()
            .copied()
            .take(BATCH_SIZE)
            .collect();
        for idx in &batch {
            self.ns_released_buf.remove(idx);
        }
        self.ipc_tx_ring.push_back(batch);
    }

    fn process_releases(&mut self) {
        if self.boot_phase != BootPhase::Ready {
            return;
        }
        if let Some(batch) = self.ipc_tx_ring.pop_front() {
            for idx in batch {
                self.rx_submitted[idx] = true;
            }
        }
    }

    fn send_packet(&mut self) {
        if self.boot_phase != BootPhase::Ready || self.phoenix_phase != PhoenixPhase::Normal {
            return;
        }
        if self.ipc_pkt_ring.len() >= RING_SIZE {
            self.dropped_tx = self.dropped_tx.saturating_add(1).min(3);
            return;
        }
        if let Some(idx) = self.tx_pool_free.iter().next().copied() {
            self.tx_pool_free.remove(&idx);
            self.tx_inflight[idx] = true;
            self.ipc_pkt_ring.push_back(idx);
        }
    }

    fn flush_tx(&mut self) {
        if let Some(idx) = self.ipc_pkt_ring.pop_front() {
            self.tx_inflight[idx] = false;
            self.tx_pool_free.insert(idx);
        }
    }

    fn socket_alloc(&mut self) {
        if self.boot_phase == BootPhase::Ready && self.socket_slots.len() < MAX_SOCKETS {
            if let Some(slot) = (0..MAX_SOCKETS).find(|idx| !self.socket_slots.contains(idx)) {
                self.socket_slots.insert(slot);
            }
        }
    }

    fn socket_close(&mut self) {
        if let Some(slot) = self.socket_slots.iter().next().copied() {
            self.socket_slots.remove(&slot);
        }
    }

    fn phoenix_cycle(&mut self) {
        if self.boot_phase != BootPhase::Ready || self.phoenix_phase != PhoenixPhase::Normal {
            return;
        }
        self.phoenix_phase = PhoenixPhase::Draining;
        while let Some(idx) = self.ipc_rx_ring.pop_front() {
            self.ns_released_buf.insert(idx);
        }
        while !self.ns_released_buf.is_empty() {
            self.flush_released();
            self.process_releases();
        }
        assert!(self.ipc_rx_ring.is_empty());
        assert!(self.ns_released_buf.is_empty());
        self.phoenix_phase = PhoenixPhase::Serialized;
        self.socket_slots.clear();
        self.phoenix_phase = PhoenixPhase::Normal;
    }

    fn assert_invariants(&self) {
        assert!(self.ipc_rx_ring.len() <= RING_SIZE);
        assert!(self.ipc_tx_ring.len() <= RING_SIZE);
        assert!(self.ipc_pkt_ring.len() <= RING_SIZE);
        assert!(self.socket_slots.len() <= MAX_SOCKETS);

        if self.boot_phase != BootPhase::Ready {
            assert!(self.ipc_rx_ring.is_empty());
        }

        for idx in 0..POOL_SIZE {
            let mut rx_locations = usize::from(self.rx_submitted[idx]);
            rx_locations += self.ipc_rx_ring.iter().filter(|&&x| x == idx).count();
            rx_locations += usize::from(self.ns_released_buf.contains(&idx));
            rx_locations += self
                .ipc_tx_ring
                .iter()
                .map(|batch| batch.iter().filter(|&&x| x == idx).count())
                .sum::<usize>();
            assert_eq!(rx_locations, 1, "RX slot {idx} must have one owner");

            assert!(
                !(self.tx_inflight[idx] && self.tx_pool_free.contains(&idx)),
                "TX slot {idx} cannot be inflight and free"
            );
            assert!(
                self.tx_pool_free.contains(&idx)
                    || self.tx_inflight[idx]
                    || self.ipc_pkt_ring.iter().any(|&x| x == idx),
                "TX slot {idx} must be free, inflight, or queued"
            );
        }
    }
}

#[test]
fn exonet_stress_preserves_tla_safety_invariants() {
    let mut model = Model::new();
    let mut rng = 0x4558_4f4e_u64;

    for step in 0..10_000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        match (rng >> 32) % 12 {
            0 => model.send_driver_init(),
            1 => model.ack_driver_init(),
            2 => model.handle_irq((rng as usize) % POOL_SIZE),
            3 => model.poll_one(),
            4 => model.flush_released(),
            5 => model.process_releases(),
            6 => model.send_packet(),
            7 => model.flush_tx(),
            8 => model.socket_alloc(),
            9 => model.socket_close(),
            10 if step % 97 == 0 => model.phoenix_cycle(),
            _ => {}
        }
        model.assert_invariants();
    }

    while !model.ipc_rx_ring.is_empty()
        || !model.ns_released_buf.is_empty()
        || !model.ipc_tx_ring.is_empty()
        || !model.ipc_pkt_ring.is_empty()
    {
        model.poll_one();
        model.flush_released();
        model.process_releases();
        model.flush_tx();
        model.assert_invariants();
    }
}
