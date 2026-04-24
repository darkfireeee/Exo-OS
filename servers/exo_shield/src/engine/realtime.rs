//! # ExoShield Realtime — Real-Time Event Monitoring
//!
//! Provides real-time monitoring of process events with configurable
//! filters, rate tracking, and alert generation. All state is stored
//! in static arrays (no heap).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::core::{
    compute_threat_score, containment_for_level, mark_process_contained, record_threat,
    score_to_level, stat_critical_inc, stat_threats_inc, update_risk_profile, ContainmentAction,
    ThreatCategory, ThreatLevel, ThreatRecord, MAX_SIG_NAME,
};

// ── Constants ───────────────────────────────────────────────────────────────

/// Maximum monitored processes.
const MAX_MONITORED_PROCS: usize = 128;

/// Maximum event filters.
const MAX_EVENT_FILTERS: usize = 64;

/// Maximum rate tracker entries.
const MAX_RATE_ENTRIES: usize = 128;

/// Maximum alerts stored.
pub const MAX_ALERTS: usize = 128;

/// Maximum alert description length.
const MAX_ALERT_DESC: usize = 48;

/// Rate tracking window in ticks.
const RATE_WINDOW_TICKS: u64 = 100;

/// Default rate thresholds.
const DEFAULT_SYSCALL_RATE_LIMIT: u32 = 10000;
const DEFAULT_NET_RATE_LIMIT: u32 = 5000;
const DEFAULT_FS_RATE_LIMIT: u32 = 8000;
const DEFAULT_ANOMALY_RATE_LIMIT: u32 = 100;

// ── Event Types ─────────────────────────────────────────────────────────────

/// Types of events the monitor can observe.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Syscall = 0,
    Network = 1,
    Filesystem = 2,
    Memory = 3,
    Process = 4,
    Ipc = 5,
    Signal = 6,
    Capability = 7,
    Custom(u8),
}

impl EventType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => EventType::Syscall,
            1 => EventType::Network,
            2 => EventType::Filesystem,
            3 => EventType::Memory,
            4 => EventType::Process,
            5 => EventType::Ipc,
            6 => EventType::Signal,
            7 => EventType::Capability,
            other => EventType::Custom(other),
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            EventType::Syscall => 0,
            EventType::Network => 1,
            EventType::Filesystem => 2,
            EventType::Memory => 3,
            EventType::Process => 4,
            EventType::Ipc => 5,
            EventType::Signal => 6,
            EventType::Capability => 7,
            EventType::Custom(v) => v,
        }
    }
}

// ── Monitored Event ─────────────────────────────────────────────────────────

/// An event observed by the real-time monitor.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MonitoredEvent {
    pub pid: u32,
    pub event_type: EventType,
    pub opcode: u32, // specific operation code
    pub arg0: u64,   // event-specific argument
    pub arg1: u64,   // event-specific argument
    pub timestamp: u64,
    pub severity: ThreatLevel,
}

impl MonitoredEvent {
    pub const fn empty() -> Self {
        MonitoredEvent {
            pid: 0,
            event_type: EventType::Syscall,
            opcode: 0,
            arg0: 0,
            arg1: 0,
            timestamp: 0,
            severity: ThreatLevel::Low,
        }
    }
}

// ── Event Filter ────────────────────────────────────────────────────────────

/// Filter rule for event monitoring.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventFilter {
    pub id: u32,
    pub event_type: EventType,
    pub pid: u32,         // 0 = all processes
    pub opcode_mask: u32, // match opcodes where (opcode & mask) != 0
    pub min_severity: ThreatLevel,
    pub action: FilterAction,
    pub enabled: bool,
}

/// Action to take when a filter matches.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    Pass = 0,       // allow, no alert
    Log = 1,        // log the event
    Alert = 2,      // generate alert
    Block = 3,      // block the event
    Quarantine = 4, // quarantine the process
}

impl FilterAction {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => FilterAction::Pass,
            1 => FilterAction::Log,
            2 => FilterAction::Alert,
            3 => FilterAction::Block,
            4 => FilterAction::Quarantine,
            _ => FilterAction::Pass,
        }
    }
}

impl EventFilter {
    pub const fn empty() -> Self {
        EventFilter {
            id: 0,
            event_type: EventType::Syscall,
            pid: 0,
            opcode_mask: 0,
            min_severity: ThreatLevel::Low,
            action: FilterAction::Log,
            enabled: false,
        }
    }

    /// Check if an event matches this filter.
    pub fn matches(&self, event: &MonitoredEvent) -> bool {
        if !self.enabled {
            return false;
        }
        // Check event type
        if event.event_type.as_u8() != self.event_type.as_u8() {
            return false;
        }
        // Check PID (0 = wildcard)
        if self.pid != 0 && event.pid != self.pid {
            return false;
        }
        // Check opcode mask
        if self.opcode_mask != 0 && (event.opcode & self.opcode_mask) == 0 {
            return false;
        }
        // Check minimum severity
        if event.severity < self.min_severity {
            return false;
        }
        true
    }
}

// ── Rate Tracker ────────────────────────────────────────────────────────────

/// Per-process rate tracking for event frequency analysis.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RateEntry {
    pub pid: u32,
    pub syscall_count: u32,
    pub net_count: u32,
    pub fs_count: u32,
    pub anomaly_count: u32,
    pub window_start: u64,
    pub syscall_rate: u32, // events per window
    pub net_rate: u32,
    pub fs_rate: u32,
    pub anomaly_rate: u32,
    pub active: bool,
}

impl RateEntry {
    pub const fn empty() -> Self {
        RateEntry {
            pid: 0,
            syscall_count: 0,
            net_count: 0,
            fs_count: 0,
            anomaly_count: 0,
            window_start: 0,
            syscall_rate: 0,
            net_rate: 0,
            fs_rate: 0,
            anomaly_rate: 0,
            active: false,
        }
    }
}

// ── Alert ───────────────────────────────────────────────────────────────────

/// An alert generated by the real-time monitor.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Alert {
    pub id: u32,
    pub pid: u32,
    pub level: ThreatLevel,
    pub category: ThreatCategory,
    pub alert_type: u8, // 0=rate, 1=filter, 2=anomaly, 3=behavioral
    pub description: [u8; MAX_ALERT_DESC],
    pub desc_len: u8,
    pub timestamp: u64,
    pub acknowledged: bool,
    pub contained: bool,
}

impl Alert {
    pub const fn empty() -> Self {
        Alert {
            id: 0,
            pid: 0,
            level: ThreatLevel::Low,
            category: ThreatCategory::None,
            alert_type: 0,
            description: [0u8; MAX_ALERT_DESC],
            desc_len: 0,
            timestamp: 0,
            acknowledged: false,
            contained: false,
        }
    }
}

// ── Event Monitor ───────────────────────────────────────────────────────────

/// The main event monitor that tracks processes and applies filters.
struct EventMonitor {
    filters: [EventFilter; MAX_EVENT_FILTERS],
    filter_count: u32,
    filter_next_id: u32,
    rates: [RateEntry; MAX_RATE_ENTRIES],
    alerts: [Alert; MAX_ALERTS],
    alert_count: u32,
    alert_next_id: u32,
    monitored: [MonitoredProc; MAX_MONITORED_PROCS],
    monitor_count: u32,
    init_done: bool,
}

/// A monitored process entry.
#[repr(C)]
#[derive(Clone, Copy)]
struct MonitoredProc {
    pid: u32,
    watch_level: ThreatLevel,
    syscall_limit: u32,
    net_limit: u32,
    fs_limit: u32,
    anomaly_limit: u32,
    active: bool,
}

impl MonitoredProc {
    const fn empty() -> Self {
        MonitoredProc {
            pid: 0,
            watch_level: ThreatLevel::Low,
            syscall_limit: DEFAULT_SYSCALL_RATE_LIMIT,
            net_limit: DEFAULT_NET_RATE_LIMIT,
            fs_limit: DEFAULT_FS_RATE_LIMIT,
            anomaly_limit: DEFAULT_ANOMALY_RATE_LIMIT,
            active: false,
        }
    }
}

impl EventMonitor {
    const fn new() -> Self {
        EventMonitor {
            filters: [EventFilter::empty(); MAX_EVENT_FILTERS],
            filter_count: 0,
            filter_next_id: 1,
            rates: [RateEntry::empty(); MAX_RATE_ENTRIES],
            alerts: [Alert::empty(); MAX_ALERTS],
            alert_count: 0,
            alert_next_id: 1,
            monitored: [MonitoredProc::empty(); MAX_MONITORED_PROCS],
            monitor_count: 0,
            init_done: false,
        }
    }

    fn init(&mut self) {
        for i in 0..MAX_EVENT_FILTERS {
            self.filters[i] = EventFilter::empty();
        }
        self.filter_count = 0;
        self.filter_next_id = 1;

        for i in 0..MAX_RATE_ENTRIES {
            self.rates[i] = RateEntry::empty();
        }
        for i in 0..MAX_ALERTS {
            self.alerts[i] = Alert::empty();
        }
        self.alert_count = 0;
        self.alert_next_id = 1;

        for i in 0..MAX_MONITORED_PROCS {
            self.monitored[i] = MonitoredProc::empty();
        }
        self.monitor_count = 0;

        // Load default filters
        self.load_default_filters();
        self.init_done = true;
    }

    fn load_default_filters(&mut self) {
        let defaults: &[EventFilter] = &[
            EventFilter {
                id: 0,
                event_type: EventType::Memory,
                pid: 0,
                opcode_mask: 0,
                min_severity: ThreatLevel::High,
                action: FilterAction::Alert,
                enabled: true,
            },
            EventFilter {
                id: 0,
                event_type: EventType::Capability,
                pid: 0,
                opcode_mask: 0xFFFF,
                min_severity: ThreatLevel::Medium,
                action: FilterAction::Alert,
                enabled: true,
            },
            EventFilter {
                id: 0,
                event_type: EventType::Process,
                pid: 0,
                opcode_mask: 0x0001, // exec
                min_severity: ThreatLevel::Medium,
                action: FilterAction::Log,
                enabled: true,
            },
            EventFilter {
                id: 0,
                event_type: EventType::Network,
                pid: 0,
                opcode_mask: 0,
                min_severity: ThreatLevel::High,
                action: FilterAction::Alert,
                enabled: true,
            },
            EventFilter {
                id: 0,
                event_type: EventType::Ipc,
                pid: 0,
                opcode_mask: 0,
                min_severity: ThreatLevel::Critical,
                action: FilterAction::Quarantine,
                enabled: true,
            },
        ];

        for filt in defaults {
            let _ = self.add_filter(filt);
        }
    }

    fn add_filter(&mut self, filter: &EventFilter) -> Option<u32> {
        if self.filter_count as usize >= MAX_EVENT_FILTERS {
            return None;
        }
        for i in 0..MAX_EVENT_FILTERS {
            if self.filters[i].id == 0 {
                let id = self.filter_next_id;
                self.filter_next_id = self.filter_next_id.wrapping_add(1);
                let mut new_filter = *filter;
                new_filter.id = id;
                self.filters[i] = new_filter;
                self.filter_count += 1;
                return Some(id);
            }
        }
        None
    }

    fn remove_filter(&mut self, id: u32) -> bool {
        for i in 0..MAX_EVENT_FILTERS {
            if self.filters[i].id == id {
                self.filters[i] = EventFilter::empty();
                self.filter_count = self.filter_count.saturating_sub(1);
                return true;
            }
        }
        false
    }

    fn add_alert(&mut self, alert: &Alert) -> Option<u32> {
        // Find empty slot or overwrite oldest acknowledged
        for i in 0..MAX_ALERTS {
            if self.alerts[i].id == 0 {
                let id = self.alert_next_id;
                self.alert_next_id = self.alert_next_id.wrapping_add(1);
                let mut new_alert = *alert;
                new_alert.id = id;
                self.alerts[i] = new_alert;
                self.alert_count = self.alert_count.saturating_add(1).min(MAX_ALERTS as u32);
                return Some(id);
            }
        }
        // Overwrite oldest acknowledged alert
        let mut oldest_idx = 0usize;
        let mut oldest_ts = u64::MAX;
        for i in 0..MAX_ALERTS {
            if self.alerts[i].acknowledged && self.alerts[i].timestamp < oldest_ts {
                oldest_ts = self.alerts[i].timestamp;
                oldest_idx = i;
            }
        }
        if oldest_ts != u64::MAX {
            let id = self.alert_next_id;
            self.alert_next_id = self.alert_next_id.wrapping_add(1);
            let mut new_alert = *alert;
            new_alert.id = id;
            self.alerts[oldest_idx] = new_alert;
            return Some(id);
        }
        None
    }

    fn acknowledge_alert(&mut self, id: u32) -> bool {
        for i in 0..MAX_ALERTS {
            if self.alerts[i].id == id {
                self.alerts[i].acknowledged = true;
                return true;
            }
        }
        false
    }

    fn get_alert(&self, id: u32) -> Option<Alert> {
        for i in 0..MAX_ALERTS {
            if self.alerts[i].id == id {
                return Some(self.alerts[i]);
            }
        }
        None
    }

    fn get_alerts_by_pid(&self, pid: u32, out: &mut [Alert], max: usize) -> usize {
        let mut written = 0usize;
        for i in 0..MAX_ALERTS {
            if written >= max {
                break;
            }
            if self.alerts[i].id != 0 && self.alerts[i].pid == pid {
                out[written] = self.alerts[i];
                written += 1;
            }
        }
        written
    }

    fn unacknowledged_count(&self) -> u32 {
        let mut c = 0u32;
        for i in 0..MAX_ALERTS {
            if self.alerts[i].id != 0 && !self.alerts[i].acknowledged {
                c += 1;
            }
        }
        c
    }

    fn register_process(&mut self, pid: u32, watch_level: ThreatLevel) -> bool {
        // Update if already registered
        for i in 0..MAX_MONITORED_PROCS {
            if self.monitored[i].pid == pid && self.monitored[i].active {
                self.monitored[i].watch_level = watch_level;
                return true;
            }
        }
        // Find empty slot
        for i in 0..MAX_MONITORED_PROCS {
            if !self.monitored[i].active {
                self.monitored[i] = MonitoredProc {
                    pid: pid,
                    watch_level: watch_level,
                    syscall_limit: DEFAULT_SYSCALL_RATE_LIMIT,
                    net_limit: DEFAULT_NET_RATE_LIMIT,
                    fs_limit: DEFAULT_FS_RATE_LIMIT,
                    anomaly_limit: DEFAULT_ANOMALY_RATE_LIMIT,
                    active: true,
                };
                self.monitor_count = self
                    .monitor_count
                    .saturating_add(1)
                    .min(MAX_MONITORED_PROCS as u32);
                return true;
            }
        }
        false
    }

    fn unregister_process(&mut self, pid: u32) -> bool {
        for i in 0..MAX_MONITORED_PROCS {
            if self.monitored[i].pid == pid && self.monitored[i].active {
                self.monitored[i].active = false;
                self.monitored[i].pid = 0;
                return true;
            }
        }
        false
    }

    fn is_monitored(&self, pid: u32) -> bool {
        for i in 0..MAX_MONITORED_PROCS {
            if self.monitored[i].pid == pid && self.monitored[i].active {
                return true;
            }
        }
        false
    }

    fn get_proc_limits(&self, pid: u32) -> Option<(u32, u32, u32, u32)> {
        for i in 0..MAX_MONITORED_PROCS {
            if self.monitored[i].pid == pid && self.monitored[i].active {
                return Some((
                    self.monitored[i].syscall_limit,
                    self.monitored[i].net_limit,
                    self.monitored[i].fs_limit,
                    self.monitored[i].anomaly_limit,
                ));
            }
        }
        None
    }

    fn set_proc_limits(&mut self, pid: u32, sysc: u32, net: u32, fs: u32, anom: u32) -> bool {
        for i in 0..MAX_MONITORED_PROCS {
            if self.monitored[i].pid == pid && self.monitored[i].active {
                self.monitored[i].syscall_limit = sysc;
                self.monitored[i].net_limit = net;
                self.monitored[i].fs_limit = fs;
                self.monitored[i].anomaly_limit = anom;
                return true;
            }
        }
        false
    }

    /// Update rate tracking for a process event.
    fn update_rate(&mut self, pid: u32, event_type: EventType, tick: u64) -> RateResult {
        let (sysc_limit, net_limit, fs_limit, anom_limit) = self.get_proc_limits(pid).unwrap_or((
            DEFAULT_SYSCALL_RATE_LIMIT,
            DEFAULT_NET_RATE_LIMIT,
            DEFAULT_FS_RATE_LIMIT,
            DEFAULT_ANOMALY_RATE_LIMIT,
        ));

        // Find or create rate entry
        let mut entry_idx = usize::MAX;
        for i in 0..MAX_RATE_ENTRIES {
            if self.rates[i].pid == pid && self.rates[i].active {
                entry_idx = i;
                break;
            }
        }
        if entry_idx == usize::MAX {
            // Create new entry
            for i in 0..MAX_RATE_ENTRIES {
                if !self.rates[i].active {
                    self.rates[i] = RateEntry {
                        pid: pid,
                        syscall_count: 0,
                        net_count: 0,
                        fs_count: 0,
                        anomaly_count: 0,
                        window_start: tick,
                        syscall_rate: 0,
                        net_rate: 0,
                        fs_rate: 0,
                        anomaly_rate: 0,
                        active: true,
                    };
                    entry_idx = i;
                    break;
                }
            }
        }

        if entry_idx == usize::MAX {
            return RateResult {
                exceeded: false,
                rate: 0,
                limit: 0,
            };
        }

        let entry = &mut self.rates[entry_idx];

        // Check if we need to roll the window
        let elapsed = tick.saturating_sub(entry.window_start);
        if elapsed >= RATE_WINDOW_TICKS {
            // Compute rates from counts
            let window_factor = if elapsed > 0 {
                RATE_WINDOW_TICKS as u32 * 1000 / (elapsed as u32)
            } else {
                1000
            };
            entry.syscall_rate = entry.syscall_count.saturating_mul(window_factor) / 1000;
            entry.net_rate = entry.net_count.saturating_mul(window_factor) / 1000;
            entry.fs_rate = entry.fs_count.saturating_mul(window_factor) / 1000;
            entry.anomaly_rate = entry.anomaly_count.saturating_mul(window_factor) / 1000;

            // Reset window
            entry.syscall_count = 0;
            entry.net_count = 0;
            entry.fs_count = 0;
            entry.anomaly_count = 0;
            entry.window_start = tick;
        }

        // Increment the relevant counter
        match event_type {
            EventType::Syscall | EventType::Signal => {
                entry.syscall_count += 1;
            }
            EventType::Network => {
                entry.net_count += 1;
            }
            EventType::Filesystem => {
                entry.fs_count += 1;
            }
            _ => {
                entry.anomaly_count += 1;
            }
        }

        let (current_rate, limit) = match event_type {
            EventType::Syscall | EventType::Signal => {
                // Estimate instantaneous rate
                let est = if elapsed > 0 {
                    entry.syscall_count as u32 * (RATE_WINDOW_TICKS as u32) / (elapsed as u32)
                } else {
                    entry.syscall_count as u32
                };
                (est, sysc_limit)
            }
            EventType::Network => {
                let est = if elapsed > 0 {
                    entry.net_count as u32 * (RATE_WINDOW_TICKS as u32) / (elapsed as u32)
                } else {
                    entry.net_count as u32
                };
                (est, net_limit)
            }
            EventType::Filesystem => {
                let est = if elapsed > 0 {
                    entry.fs_count as u32 * (RATE_WINDOW_TICKS as u32) / (elapsed as u32)
                } else {
                    entry.fs_count as u32
                };
                (est, fs_limit)
            }
            _ => {
                let est = if elapsed > 0 {
                    entry.anomaly_count as u32 * (RATE_WINDOW_TICKS as u32) / (elapsed as u32)
                } else {
                    entry.anomaly_count as u32
                };
                (est, anom_limit)
            }
        };

        RateResult {
            exceeded: current_rate > limit,
            rate: current_rate,
            limit: limit,
        }
    }

    /// Get rate data for a process.
    fn get_rate_entry(&self, pid: u32) -> Option<RateEntry> {
        for i in 0..MAX_RATE_ENTRIES {
            if self.rates[i].pid == pid && self.rates[i].active {
                return Some(self.rates[i]);
            }
        }
        None
    }

    /// Process an incoming event through filters and rate tracking.
    fn process_event(&mut self, event: &MonitoredEvent, tick: u64) -> EventProcessingResult {
        let mut result = EventProcessingResult {
            action_taken: FilterAction::Pass,
            alert_id: 0,
            rate_exceeded: false,
            contained: false,
        };

        // Step 1: Rate tracking
        let rate_result = self.update_rate(event.pid, event.event_type, tick);
        result.rate_exceeded = rate_result.exceeded;

        // Step 2: Apply event filters
        let mut best_action = FilterAction::Pass;
        for i in 0..MAX_EVENT_FILTERS {
            if self.filters[i].id != 0 && self.filters[i].matches(event) {
                if self.filters[i].action as u8 > best_action as u8 {
                    best_action = self.filters[i].action;
                }
            }
        }

        // Step 3: Rate exceeded escalation
        if rate_result.exceeded && (best_action as u8) < (FilterAction::Alert as u8) {
            best_action = FilterAction::Alert;
        }

        result.action_taken = best_action;

        // Step 4: Execute action
        match best_action {
            FilterAction::Pass => {}
            FilterAction::Log => {
                // Just update risk profile (no alert)
                update_risk_profile(event.pid, rate_result.rate, 0, 0, 0, tick);
            }
            FilterAction::Alert => {
                self.generate_alert(
                    event.pid,
                    event.severity,
                    ThreatCategory::Anomaly,
                    if rate_result.exceeded { 1 } else { 0 },
                    b"rate_threshold_exceeded",
                    tick,
                );
                update_risk_profile(
                    event.pid,
                    rate_result.rate,
                    rate_result.rate, // pass rate as anomaly indicator
                    0,
                    1, // one anomaly
                    tick,
                );
            }
            FilterAction::Block => {
                let alert_id = self.generate_alert(
                    event.pid,
                    ThreatLevel::High,
                    ThreatCategory::PolicyViol,
                    2,
                    b"event_blocked_policy",
                    tick,
                );
                result.alert_id = alert_id.unwrap_or(0);
                update_risk_profile(event.pid, rate_result.rate, 0, 0, 2, tick);
            }
            FilterAction::Quarantine => {
                let alert_id = self.generate_alert(
                    event.pid,
                    ThreatLevel::Critical,
                    ThreatCategory::SandboxEscape,
                    3,
                    b"quarantine_triggered",
                    tick,
                );
                result.alert_id = alert_id.unwrap_or(0);
                mark_process_contained(event.pid, tick);
                result.contained = true;
                // Record as critical threat
                let mut rec = ThreatRecord::empty();
                rec.pid = event.pid;
                rec.level = ThreatLevel::Critical;
                rec.category = ThreatCategory::SandboxEscape;
                rec.score = 900;
                rec.timestamp = tick;
                rec.contained = true;
                let _ = record_threat(&rec);
                stat_threats_inc();
                stat_critical_inc();
            }
        }

        result
    }

    fn generate_alert(
        &mut self,
        pid: u32,
        level: ThreatLevel,
        category: ThreatCategory,
        alert_type: u8,
        desc: &[u8],
        tick: u64,
    ) -> Option<u32> {
        let mut alert = Alert::empty();
        alert.pid = pid;
        alert.level = level;
        alert.category = category;
        alert.alert_type = alert_type;
        let dlen = desc.len().min(MAX_ALERT_DESC);
        alert.description[..dlen].copy_from_slice(&desc[..dlen]);
        alert.desc_len = dlen as u8;
        alert.timestamp = tick;
        alert.acknowledged = false;
        alert.contained = false;

        let id = self.add_alert(&alert);
        if let Some(aid) = id {
            STATS_ALERTS_GENERATED.fetch_add(1, Ordering::Relaxed);
        }
        id
    }
}

// ── Internal Types ──────────────────────────────────────────────────────────

struct RateResult {
    exceeded: bool,
    rate: u32,
    limit: u32,
}

struct EventProcessingResult {
    action_taken: FilterAction,
    alert_id: u32,
    rate_exceeded: bool,
    contained: bool,
}

// ── Global State ────────────────────────────────────────────────────────────

static EVENT_MONITOR: Mutex<EventMonitor> = Mutex::new(EventMonitor::new());

static STATS_EVENTS_PROCESSED: AtomicU64 = AtomicU64::new(0);
static STATS_EVENTS_FILTERED: AtomicU64 = AtomicU64::new(0);
static STATS_RATE_EXCEEDED: AtomicU64 = AtomicU64::new(0);
static STATS_ALERTS_GENERATED: AtomicU64 = AtomicU64::new(0);
static STATS_PROCESSES_CONTAINED: AtomicU64 = AtomicU64::new(0);
static STATS_REALTIME_INIT: AtomicBool = AtomicBool::new(false);

// ── Public API ──────────────────────────────────────────────────────────────

/// Initialize the real-time monitoring subsystem.
pub fn realtime_init() {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.init();
    STATS_REALTIME_INIT.store(true, Ordering::Release);
}

/// Check if the real-time monitor is initialized.
pub fn realtime_is_init() -> bool {
    STATS_REALTIME_INIT.load(Ordering::Acquire)
}

/// Process an incoming event through the monitoring pipeline.
/// Returns the alert ID if an alert was generated (0 otherwise),
/// and whether the process was contained.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventProcessResult {
    pub alert_id: u32,
    pub action: FilterAction,
    pub rate_exceeded: bool,
    pub contained: bool,
}

/// Submit an event for real-time processing.
pub fn submit_event(event: &MonitoredEvent, tick: u64) -> EventProcessResult {
    STATS_EVENTS_PROCESSED.fetch_add(1, Ordering::Relaxed);

    let mut monitor = EVENT_MONITOR.lock();
    let result = monitor.process_event(event, tick);

    STATS_EVENTS_FILTERED.fetch_add(1, Ordering::Relaxed);
    if result.rate_exceeded {
        STATS_RATE_EXCEEDED.fetch_add(1, Ordering::Relaxed);
    }
    if result.contained {
        STATS_PROCESSES_CONTAINED.fetch_add(1, Ordering::Relaxed);
    }

    EventProcessResult {
        alert_id: result.alert_id,
        action: result.action_taken,
        rate_exceeded: result.rate_exceeded,
        contained: result.contained,
    }
}

/// Register a process for monitoring.
pub fn monitor_process(pid: u32, watch_level: ThreatLevel) -> bool {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.register_process(pid, watch_level)
}

/// Unregister a process from monitoring.
pub fn unmonitor_process(pid: u32) -> bool {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.unregister_process(pid)
}

/// Check if a process is being monitored.
pub fn is_process_monitored(pid: u32) -> bool {
    let monitor = EVENT_MONITOR.lock();
    monitor.is_monitored(pid)
}

/// Set rate limits for a monitored process.
pub fn set_process_rate_limits(
    pid: u32,
    syscall_limit: u32,
    net_limit: u32,
    fs_limit: u32,
    anomaly_limit: u32,
) -> bool {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.set_proc_limits(pid, syscall_limit, net_limit, fs_limit, anomaly_limit)
}

/// Get the current rate entry for a process.
pub fn get_process_rate(pid: u32) -> Option<RateEntry> {
    let monitor = EVENT_MONITOR.lock();
    monitor.get_rate_entry(pid)
}

/// Add an event filter.
pub fn add_event_filter(filter: &EventFilter) -> Option<u32> {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.add_filter(filter)
}

/// Remove an event filter by ID.
pub fn remove_event_filter(id: u32) -> bool {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.remove_filter(id)
}

/// Get an alert by ID.
pub fn get_alert(id: u32) -> Option<Alert> {
    let monitor = EVENT_MONITOR.lock();
    monitor.get_alert(id)
}

/// Get all alerts for a PID. Writes up to `max` entries into `out`.
/// Returns the number written.
pub fn get_alerts_for_pid(pid: u32, out: &mut [Alert], max: usize) -> usize {
    let monitor = EVENT_MONITOR.lock();
    monitor.get_alerts_by_pid(pid, out, max)
}

/// Acknowledge an alert.
pub fn acknowledge_alert(id: u32) -> bool {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.acknowledge_alert(id)
}

/// Count unacknowledged alerts.
pub fn unacknowledged_alert_count() -> u32 {
    let monitor = EVENT_MONITOR.lock();
    monitor.unacknowledged_count()
}

/// Generate a manual alert (e.g., from scanner results).
pub fn generate_manual_alert(
    pid: u32,
    level: ThreatLevel,
    category: ThreatCategory,
    alert_type: u8,
    desc: &[u8],
    tick: u64,
) -> Option<u32> {
    let mut monitor = EVENT_MONITOR.lock();
    monitor.generate_alert(pid, level, category, alert_type, desc, tick)
}

// ── Real-time Monitoring Statistics ─────────────────────────────────────────

/// Real-time monitoring statistics.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RealtimeStats {
    pub events_processed: u64,
    pub events_filtered: u64,
    pub rate_exceeded: u64,
    pub alerts_generated: u64,
    pub processes_contained: u64,
    pub unacknowledged_alerts: u32,
}

/// Retrieve current real-time monitoring statistics.
pub fn get_realtime_stats() -> RealtimeStats {
    RealtimeStats {
        events_processed: STATS_EVENTS_PROCESSED.load(Ordering::Relaxed),
        events_filtered: STATS_EVENTS_FILTERED.load(Ordering::Relaxed),
        rate_exceeded: STATS_RATE_EXCEEDED.load(Ordering::Relaxed),
        alerts_generated: STATS_ALERTS_GENERATED.load(Ordering::Relaxed),
        processes_contained: STATS_PROCESSES_CONTAINED.load(Ordering::Relaxed),
        unacknowledged_alerts: unacknowledged_alert_count(),
    }
}
