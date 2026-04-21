const MAX_DOMAINS: usize = 64;

#[derive(Clone, Copy)]
struct DomainRecord {
    active: bool,
    pid: u32,
    domain_hint: u32,
    fault_count: u32,
    last_fault_code: u32,
    last_fault_value0: u64,
    last_fault_value1: u64,
}

impl DomainRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            pid: 0,
            domain_hint: 0,
            fault_count: 0,
            last_fault_code: 0,
            last_fault_value0: 0,
            last_fault_value1: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct DomainSnapshot {
    pub domain_hint: u32,
    pub fault_count: u32,
    pub last_fault_code: u32,
    pub last_fault_value0: u64,
    pub last_fault_value1: u64,
}

pub struct IommuLedger {
    domains: [DomainRecord; MAX_DOMAINS],
}

impl IommuLedger {
    pub const fn new() -> Self {
        Self {
            domains: [DomainRecord::empty(); MAX_DOMAINS],
        }
    }

    pub fn bind_driver(&mut self, pid: u32, domain_hint: u32) {
        if let Some(idx) = self.domains.iter().position(|entry| entry.active && entry.pid == pid) {
            self.domains[idx].domain_hint = domain_hint;
            return;
        }

        if let Some(idx) = self.domains.iter().position(|entry| !entry.active) {
            self.domains[idx] = DomainRecord {
                active: true,
                pid,
                domain_hint,
                fault_count: 0,
                last_fault_code: 0,
                last_fault_value0: 0,
                last_fault_value1: 0,
            };
        }
    }

    pub fn unbind_driver(&mut self, pid: u32) {
        if let Some(idx) = self.domains.iter().position(|entry| entry.active && entry.pid == pid) {
            self.domains[idx] = DomainRecord::empty();
        }
    }

    pub fn report_fault(&mut self, pid: u32, fault_code: u32, value0: u64, value1: u64) {
        if let Some(idx) = self.domains.iter().position(|entry| entry.active && entry.pid == pid) {
            let entry = &mut self.domains[idx];
            entry.fault_count = entry.fault_count.saturating_add(1);
            entry.last_fault_code = fault_code;
            entry.last_fault_value0 = value0;
            entry.last_fault_value1 = value1;
        }
    }

    pub fn snapshot(&self, pid: u32) -> Option<DomainSnapshot> {
        let entry = self.domains.iter().find(|entry| entry.active && entry.pid == pid)?;
        Some(DomainSnapshot {
            domain_hint: entry.domain_hint,
            fault_count: entry.fault_count,
            last_fault_code: entry.last_fault_code,
            last_fault_value0: entry.last_fault_value0,
            last_fault_value1: entry.last_fault_value1,
        })
    }
}
