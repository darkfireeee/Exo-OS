const MAX_EVENTS: usize = 64;

#[repr(u32)]
#[derive(Clone, Copy)]
pub enum DeviceEventKind {
    Registered = 0,
    Claimed = 1,
    Released = 2,
    Faulted = 3,
    PowerChanged = 4,
}

#[derive(Clone, Copy)]
pub struct DeviceEvent {
    pub kind: DeviceEventKind,
    pub bdf_raw: u32,
    pub pid: u32,
    pub value: u64,
}

impl DeviceEvent {
    pub const fn new(kind: DeviceEventKind, bdf_raw: u32, pid: u32, value: u64) -> Self {
        Self {
            kind,
            bdf_raw,
            pid,
            value,
        }
    }
}

#[derive(Clone, Copy)]
struct QueueSlot {
    active: bool,
    event: DeviceEvent,
}

impl QueueSlot {
    const fn empty() -> Self {
        Self {
            active: false,
            event: DeviceEvent::new(DeviceEventKind::Registered, 0, 0, 0),
        }
    }
}

pub struct HotplugQueue {
    head: usize,
    tail: usize,
    slots: [QueueSlot; MAX_EVENTS],
}

impl HotplugQueue {
    pub const fn new() -> Self {
        Self {
            head: 0,
            tail: 0,
            slots: [QueueSlot::empty(); MAX_EVENTS],
        }
    }

    pub fn push(&mut self, event: DeviceEvent) {
        let idx = self.head % MAX_EVENTS;
        self.slots[idx] = QueueSlot {
            active: true,
            event,
        };
        self.head = self.head.wrapping_add(1);
        if self.head.wrapping_sub(self.tail) > MAX_EVENTS {
            self.tail = self.head - MAX_EVENTS;
        }
    }

    pub fn pop(&mut self) -> Option<DeviceEvent> {
        if self.tail == self.head {
            return None;
        }

        let idx = self.tail % MAX_EVENTS;
        let slot = self.slots[idx];
        self.slots[idx] = QueueSlot::empty();
        self.tail = self.tail.wrapping_add(1);

        if slot.active {
            Some(slot.event)
        } else {
            None
        }
    }
}
