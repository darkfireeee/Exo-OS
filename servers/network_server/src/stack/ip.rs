use exo_syscall_abi as syscall;

const MAX_ROUTES: usize = 32;

#[derive(Clone, Copy)]
struct RouteRecord {
    active: bool,
    destination: u32,
    prefix_len: u8,
    next_hop: u32,
    metric: u16,
    interface_id: u16,
    flags: u32,
}

impl RouteRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            destination: 0,
            prefix_len: 0,
            next_hop: 0,
            metric: 0,
            interface_id: 0,
            flags: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct RouteSnapshot {
    pub destination: u32,
    pub prefix_len: u8,
    pub next_hop: u32,
    pub metric: u16,
    pub interface_id: u16,
    pub flags: u32,
}

pub struct RouteTable {
    routes: [RouteRecord; MAX_ROUTES],
}

impl RouteTable {
    pub const fn new() -> Self {
        Self {
            routes: [RouteRecord::empty(); MAX_ROUTES],
        }
    }

    pub fn add_route(
        &mut self,
        destination: u32,
        prefix_len: u8,
        next_hop: u32,
        metric: u16,
        interface_id: u16,
        flags: u32,
    ) -> Result<RouteSnapshot, i64> {
        if prefix_len > 32 {
            return Err(syscall::EINVAL);
        }

        if let Some(idx) = self.routes.iter().position(|entry| {
            entry.active && entry.destination == destination && entry.prefix_len == prefix_len
        }) {
            self.routes[idx] = RouteRecord {
                active: true,
                destination,
                prefix_len,
                next_hop,
                metric,
                interface_id,
                flags,
            };
            return Ok(self.snapshot(idx));
        }

        let Some(idx) = self.routes.iter().position(|entry| !entry.active) else {
            return Err(syscall::ENOSPC);
        };

        self.routes[idx] = RouteRecord {
            active: true,
            destination,
            prefix_len,
            next_hop,
            metric,
            interface_id,
            flags,
        };
        Ok(self.snapshot(idx))
    }

    pub fn lookup(&self, target: u32) -> Option<RouteSnapshot> {
        let mut best_idx = None;
        let mut best_prefix = 0u8;

        let mut idx = 0usize;
        while idx < self.routes.len() {
            let route = self.routes[idx];
            if route.active && prefix_match(route.destination, target, route.prefix_len) && route.prefix_len >= best_prefix {
                best_idx = Some(idx);
                best_prefix = route.prefix_len;
            }
            idx += 1;
        }

        best_idx.map(|idx| self.snapshot(idx))
    }

    pub fn count(&self) -> u32 {
        self.routes.iter().filter(|route| route.active).count() as u32
    }

    fn snapshot(&self, idx: usize) -> RouteSnapshot {
        let route = self.routes[idx];
        RouteSnapshot {
            destination: route.destination,
            prefix_len: route.prefix_len,
            next_hop: route.next_hop,
            metric: route.metric,
            interface_id: route.interface_id,
            flags: route.flags,
        }
    }
}

fn prefix_match(lhs: u32, rhs: u32, prefix_len: u8) -> bool {
    if prefix_len == 0 {
        return true;
    }
    let shift = 32u32.saturating_sub(prefix_len as u32);
    let mask = u32::MAX.checked_shl(shift).unwrap_or(0);
    (lhs & mask) == (rhs & mask)
}
