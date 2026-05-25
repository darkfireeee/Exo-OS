pub const MAX_ROUTES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteEntry {
    pub dest_net: u32,
    pub prefix_len: u8,
    pub gateway: u32,
    pub metric: u8,
}

impl RouteEntry {
    pub const fn empty() -> Self {
        Self {
            dest_net: 0,
            prefix_len: 0,
            gateway: 0,
            metric: u8::MAX,
        }
    }

    fn matches(&self, dst_ip: u32) -> bool {
        (dst_ip & mask(self.prefix_len)) == (self.dest_net & mask(self.prefix_len))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteError {
    InvalidPrefix,
    Full,
}

pub struct RouteTable {
    entries: [RouteEntry; MAX_ROUTES],
    count: usize,
}

impl RouteTable {
    pub const fn new() -> Self {
        Self {
            entries: [RouteEntry::empty(); MAX_ROUTES],
            count: 0,
        }
    }

    pub fn clear(&mut self) {
        self.entries = [RouteEntry::empty(); MAX_ROUTES];
        self.count = 0;
    }

    pub fn add(
        &mut self,
        dest_net: u32,
        prefix_len: u8,
        gateway: u32,
        metric: u8,
    ) -> Result<(), RouteError> {
        if prefix_len > 32 {
            return Err(RouteError::InvalidPrefix);
        }

        let normalized = RouteEntry {
            dest_net: dest_net & mask(prefix_len),
            prefix_len,
            gateway,
            metric,
        };

        let mut idx = 0usize;
        while idx < self.count {
            if self.entries[idx].dest_net == normalized.dest_net
                && self.entries[idx].prefix_len == normalized.prefix_len
                && self.entries[idx].metric == normalized.metric
            {
                self.entries[idx] = normalized;
                return Ok(());
            }
            idx += 1;
        }

        if self.count >= self.entries.len() {
            return Err(RouteError::Full);
        }
        self.entries[self.count] = normalized;
        self.count += 1;
        Ok(())
    }

    pub fn lookup(&self, dst_ip: u32) -> Option<u32> {
        let mut best: Option<RouteEntry> = None;
        let mut idx = 0usize;
        while idx < self.count {
            let candidate = self.entries[idx];
            if candidate.matches(dst_ip)
                && best.map_or(true, |current| {
                    candidate.prefix_len > current.prefix_len
                        || (candidate.prefix_len == current.prefix_len
                            && candidate.metric < current.metric)
                })
            {
                best = Some(candidate);
            }
            idx += 1;
        }
        best.map(|route| {
            if route.gateway == 0 {
                dst_ip
            } else {
                route.gateway
            }
        })
    }

    pub fn default_gateway(&self) -> Option<u32> {
        let mut best: Option<RouteEntry> = None;
        let mut idx = 0usize;
        while idx < self.count {
            let candidate = self.entries[idx];
            if candidate.prefix_len == 0
                && best.map_or(true, |current| candidate.metric < current.metric)
            {
                best = Some(candidate);
            }
            idx += 1;
        }
        best.and_then(|route| (route.gateway != 0).then_some(route.gateway))
    }

    pub const fn len(&self) -> usize {
        self.count
    }
}

pub const fn mask(prefix_len: u8) -> u32 {
    if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_len)
    }
}
