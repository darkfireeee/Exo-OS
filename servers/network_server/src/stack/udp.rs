use exo_syscall_abi as syscall;

#[derive(Clone, Copy)]
pub struct UdpBudget {
    pub max_datagram: u32,
    pub queue_slots: u16,
}

pub struct UdpPortAllocator {
    next_port: u16,
    budget: UdpBudget,
}

impl UdpPortAllocator {
    pub const fn new() -> Self {
        Self {
            next_port: 49_152,
            budget: UdpBudget {
                max_datagram: 1472,
                queue_slots: 64,
            },
        }
    }

    pub fn allocate(&mut self) -> u16 {
        let port = self.next_port;
        self.next_port = if self.next_port >= 65_534 {
            49_152
        } else {
            self.next_port + 1
        };
        port
    }

    pub fn validate_len(&self, len: u32) -> Result<(), i64> {
        if len > self.budget.max_datagram {
            Err(syscall::E2BIG)
        } else {
            Ok(())
        }
    }

    pub const fn budget(&self) -> UdpBudget {
        self.budget
    }
}
