use exo_syscall_abi as syscall;

use crate::policy_advisor::SchedulingClass;

const MAX_THREADS: usize = 128;

#[derive(Clone, Copy)]
struct ThreadRecord {
    active: bool,
    pid: u32,
    tid: u32,
    nice: i8,
    priority_weight: u32,
    class: SchedulingClass,
    affinity_mask: u64,
    flags: u32,
}

impl ThreadRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            pid: 0,
            tid: 0,
            nice: 0,
            priority_weight: 1024,
            class: SchedulingClass::Cfs,
            affinity_mask: 1,
            flags: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ThreadSnapshot {
    pub pid: u32,
    pub tid: u32,
    pub nice: i8,
    pub priority_weight: u32,
    pub class: SchedulingClass,
    pub affinity_mask: u64,
    pub flags: u32,
}

pub struct ThreadTable {
    records: [ThreadRecord; MAX_THREADS],
}

impl ThreadTable {
    pub const fn new() -> Self {
        Self {
            records: [ThreadRecord::empty(); MAX_THREADS],
        }
    }

    pub fn register(
        &mut self,
        pid: u32,
        tid: u32,
        nice: i8,
        class: SchedulingClass,
        affinity_mask: u64,
        priority_weight: u32,
        flags: u32,
    ) -> Result<ThreadSnapshot, i64> {
        if let Some(idx) = self
            .records
            .iter()
            .position(|record| record.active && record.tid == tid)
        {
            self.records[idx] = ThreadRecord {
                active: true,
                pid,
                tid,
                nice,
                priority_weight,
                class,
                affinity_mask: affinity_mask.max(1),
                flags,
            };
            return Ok(self.snapshot(idx));
        }

        let Some(idx) = self.records.iter().position(|record| !record.active) else {
            return Err(syscall::ENOSPC);
        };

        self.records[idx] = ThreadRecord {
            active: true,
            pid,
            tid,
            nice,
            priority_weight,
            class,
            affinity_mask: affinity_mask.max(1),
            flags,
        };
        Ok(self.snapshot(idx))
    }

    pub fn update_priority(
        &mut self,
        pid: u32,
        tid: u32,
        nice: i8,
        priority_weight: u32,
    ) -> Result<ThreadSnapshot, i64> {
        let idx = self.lookup_owned(pid, tid)?;
        self.records[idx].nice = nice;
        self.records[idx].priority_weight = priority_weight;
        Ok(self.snapshot(idx))
    }

    pub fn update_class(
        &mut self,
        pid: u32,
        tid: u32,
        class: SchedulingClass,
        flags: u32,
    ) -> Result<ThreadSnapshot, i64> {
        let idx = self.lookup_owned(pid, tid)?;
        self.records[idx].class = class;
        self.records[idx].flags = flags;
        Ok(self.snapshot(idx))
    }

    pub fn set_affinity(
        &mut self,
        pid: u32,
        tid: u32,
        affinity_mask: u64,
    ) -> Result<ThreadSnapshot, i64> {
        let idx = self.lookup_owned(pid, tid)?;
        self.records[idx].affinity_mask = affinity_mask.max(1);
        Ok(self.snapshot(idx))
    }

    pub fn snapshot_owned(&self, pid: u32, tid: u32) -> Result<ThreadSnapshot, i64> {
        let idx = self.lookup_owned(pid, tid)?;
        Ok(self.snapshot(idx))
    }

    pub fn snapshot_any(&self, tid: u32) -> Option<ThreadSnapshot> {
        let idx = self
            .records
            .iter()
            .position(|record| record.active && record.tid == tid)?;
        Some(self.snapshot(idx))
    }

    pub fn owner_pid(&self, tid: u32) -> Option<u32> {
        self.records
            .iter()
            .find(|record| record.active && record.tid == tid)
            .map(|record| record.pid)
    }

    pub fn active_count(&self) -> u32 {
        self.records.iter().filter(|record| record.active).count() as u32
    }

    fn lookup_owned(&self, pid: u32, tid: u32) -> Result<usize, i64> {
        let Some(idx) = self
            .records
            .iter()
            .position(|record| record.active && record.tid == tid)
        else {
            return Err(syscall::ENOENT);
        };
        if self.records[idx].pid != pid {
            return Err(syscall::EPERM);
        }
        Ok(idx)
    }

    fn snapshot(&self, idx: usize) -> ThreadSnapshot {
        let record = self.records[idx];
        ThreadSnapshot {
            pid: record.pid,
            tid: record.tid,
            nice: record.nice,
            priority_weight: record.priority_weight,
            class: record.class,
            affinity_mask: record.affinity_mask,
            flags: record.flags,
        }
    }
}
