use exo_syscall_abi as syscall;

const MAX_RT_THREADS: usize = 32;
const UTILIZATION_LIMIT_PPM: u32 = 950_000;

#[derive(Clone, Copy)]
struct RtRecord {
    active: bool,
    tid: u32,
    runtime_us: u32,
    period_us: u32,
    utilization_ppm: u32,
}

impl RtRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            tid: 0,
            runtime_us: 0,
            period_us: 0,
            utilization_ppm: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct RtSnapshot {
    pub tid: u32,
    pub runtime_us: u32,
    pub period_us: u32,
    pub utilization_ppm: u32,
    pub total_utilization_ppm: u32,
}

pub struct RealtimeAdmission {
    records: [RtRecord; MAX_RT_THREADS],
    total_utilization_ppm: u32,
}

impl RealtimeAdmission {
    pub const fn new() -> Self {
        Self {
            records: [RtRecord::empty(); MAX_RT_THREADS],
            total_utilization_ppm: 0,
        }
    }

    pub fn admit(&mut self, tid: u32, runtime_us: u32, period_us: u32) -> Result<RtSnapshot, i64> {
        if runtime_us == 0 || period_us == 0 || runtime_us > period_us {
            return Err(syscall::EINVAL);
        }

        let utilization_ppm = ((runtime_us as u64).saturating_mul(1_000_000) / period_us as u64) as u32;
        let existing = self.records.iter().find(|record| record.active && record.tid == tid).copied();
        let previous_util = existing.map(|record| record.utilization_ppm).unwrap_or(0);
        let next_total = self
            .total_utilization_ppm
            .saturating_sub(previous_util)
            .saturating_add(utilization_ppm);
        if next_total > UTILIZATION_LIMIT_PPM {
            return Err(syscall::EBUSY);
        }

        if let Some(idx) = self.records.iter().position(|record| record.active && record.tid == tid) {
            self.records[idx] = RtRecord {
                active: true,
                tid,
                runtime_us,
                period_us,
                utilization_ppm,
            };
            self.total_utilization_ppm = next_total;
            return Ok(self.snapshot(tid).unwrap());
        }

        let Some(idx) = self.records.iter().position(|record| !record.active) else {
            return Err(syscall::ENOSPC);
        };

        self.records[idx] = RtRecord {
            active: true,
            tid,
            runtime_us,
            period_us,
            utilization_ppm,
        };
        self.total_utilization_ppm = next_total;
        Ok(self.snapshot(tid).unwrap())
    }

    pub fn release(&mut self, tid: u32) -> Option<RtSnapshot> {
        let idx = self.records.iter().position(|record| record.active && record.tid == tid)?;
        let record = self.records[idx];
        self.total_utilization_ppm = self.total_utilization_ppm.saturating_sub(record.utilization_ppm);
        self.records[idx] = RtRecord::empty();
        Some(RtSnapshot {
            tid: record.tid,
            runtime_us: record.runtime_us,
            period_us: record.period_us,
            utilization_ppm: record.utilization_ppm,
            total_utilization_ppm: self.total_utilization_ppm,
        })
    }

    pub fn snapshot(&self, tid: u32) -> Option<RtSnapshot> {
        let record = self.records.iter().find(|entry| entry.active && entry.tid == tid)?;
        Some(RtSnapshot {
            tid: record.tid,
            runtime_us: record.runtime_us,
            period_us: record.period_us,
            utilization_ppm: record.utilization_ppm,
            total_utilization_ppm: self.total_utilization_ppm,
        })
    }

    pub const fn total_utilization_ppm(&self) -> u32 {
        self.total_utilization_ppm
    }
}
