use crate::policy_advisor::SchedulingClass;
use crate::thread_table::ThreadSnapshot;

const MAX_STATS: usize = 128;

#[derive(Clone, Copy)]
struct StatsRecord {
    active: bool,
    pid: u32,
    tid: u32,
    class: SchedulingClass,
    priority_weight: u32,
    affinity_mask: u64,
    yield_count: u32,
    priority_updates: u32,
    policy_updates: u32,
    affinity_updates: u32,
    last_error: i64,
}

impl StatsRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            pid: 0,
            tid: 0,
            class: SchedulingClass::Cfs,
            priority_weight: 1024,
            affinity_mask: 1,
            yield_count: 0,
            priority_updates: 0,
            policy_updates: 0,
            affinity_updates: 0,
            last_error: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct StatsSnapshot {
    pub pid: u32,
    pub tid: u32,
    pub class: SchedulingClass,
    pub priority_weight: u32,
    pub affinity_mask: u64,
    pub yield_count: u32,
    pub priority_updates: u32,
    pub policy_updates: u32,
    pub affinity_updates: u32,
    pub last_error: i64,
}

pub struct StatsCollector {
    records: [StatsRecord; MAX_STATS],
}

impl StatsCollector {
    pub const fn new() -> Self {
        Self {
            records: [StatsRecord::empty(); MAX_STATS],
        }
    }

    pub fn note_register(&mut self, thread: ThreadSnapshot) {
        let idx = self.ensure_slot(thread.pid, thread.tid);
        self.records[idx].active = true;
        self.records[idx].pid = thread.pid;
        self.records[idx].tid = thread.tid;
        self.records[idx].class = thread.class;
        self.records[idx].priority_weight = thread.priority_weight;
        self.records[idx].affinity_mask = thread.affinity_mask;
    }

    pub fn note_priority(&mut self, thread: ThreadSnapshot) {
        let idx = self.ensure_slot(thread.pid, thread.tid);
        self.records[idx].priority_weight = thread.priority_weight;
        self.records[idx].class = thread.class;
        self.records[idx].priority_updates = self.records[idx].priority_updates.saturating_add(1);
    }

    pub fn note_policy(&mut self, thread: ThreadSnapshot) {
        let idx = self.ensure_slot(thread.pid, thread.tid);
        self.records[idx].class = thread.class;
        self.records[idx].priority_weight = thread.priority_weight;
        self.records[idx].policy_updates = self.records[idx].policy_updates.saturating_add(1);
    }

    pub fn note_affinity(&mut self, thread: ThreadSnapshot) {
        let idx = self.ensure_slot(thread.pid, thread.tid);
        self.records[idx].affinity_mask = thread.affinity_mask;
        self.records[idx].affinity_updates = self.records[idx].affinity_updates.saturating_add(1);
    }

    pub fn note_yield(&mut self, pid: u32, tid: u32) {
        let idx = self.ensure_slot(pid, tid);
        self.records[idx].yield_count = self.records[idx].yield_count.saturating_add(1);
    }

    pub fn note_error(&mut self, pid: u32, tid: u32, err: i64) {
        let idx = self.ensure_slot(pid, tid);
        self.records[idx].last_error = err;
    }

    pub fn snapshot(&self, tid: u32) -> Option<StatsSnapshot> {
        let record = self
            .records
            .iter()
            .find(|entry| entry.active && entry.tid == tid)?;
        Some(StatsSnapshot {
            pid: record.pid,
            tid: record.tid,
            class: record.class,
            priority_weight: record.priority_weight,
            affinity_mask: record.affinity_mask,
            yield_count: record.yield_count,
            priority_updates: record.priority_updates,
            policy_updates: record.policy_updates,
            affinity_updates: record.affinity_updates,
            last_error: record.last_error,
        })
    }

    pub fn active_count(&self) -> u32 {
        self.records.iter().filter(|entry| entry.active).count() as u32
    }

    fn ensure_slot(&mut self, pid: u32, tid: u32) -> usize {
        if let Some(idx) = self
            .records
            .iter()
            .position(|entry| entry.active && entry.tid == tid)
        {
            return idx;
        }
        if let Some(idx) = self.records.iter().position(|entry| !entry.active) {
            self.records[idx].active = true;
            self.records[idx].pid = pid;
            self.records[idx].tid = tid;
            return idx;
        }
        0
    }
}
