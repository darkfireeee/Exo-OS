//! Time System Call Handlers
//!
//! Handles time-related operations

use crate::memory::{MemoryResult, MemoryError};

/// Clock ID
#[derive(Debug, Clone, Copy)]
pub enum ClockId {
    Realtime = 0,
    Monotonic = 1,
    ProcessCpu = 2,
    ThreadCpu = 3,
    Boottime = 4,
}

/// Timespec structure
#[derive(Debug, Clone, Copy)]
pub struct TimeSpec {
    pub seconds: i64,
    pub nanoseconds: i64,
}

impl TimeSpec {
    pub const fn new(seconds: i64, nanoseconds: i64) -> Self {
        Self { seconds, nanoseconds }
    }
    
    pub const fn zero() -> Self {
        Self::new(0, 0)
    }
    
    pub fn as_nanos(&self) -> i64 {
        self.seconds * 1_000_000_000 + self.nanoseconds
    }
    
    pub fn from_nanos(nanos: i64) -> Self {
        Self {
            seconds: nanos / 1_000_000_000,
            nanoseconds: nanos % 1_000_000_000,
        }
    }
}

/// Timer ID
pub type TimerId = u64;

/// Get time from clock
pub fn sys_clock_gettime(clock_id: ClockId) -> MemoryResult<TimeSpec> {
    log::debug!("sys_clock_gettime: clock_id={:?}", clock_id);
    
    match clock_id {
        ClockId::Realtime => {
            // Get UNIX timestamp (seconds since 1970-01-01)
            let unix_ts = crate::time::unix_timestamp();
            let uptime_ns = crate::time::uptime_ns();
            let ns = uptime_ns % 1_000_000_000;
            Ok(TimeSpec::new(unix_ts as i64, ns as i64))
        }
        ClockId::Monotonic | ClockId::Boottime => {
            // Get monotonic time since boot
            let uptime_ns = crate::time::uptime_ns();
            Ok(TimeSpec::from_nanos(uptime_ns as i64))
        }
        ClockId::ProcessCpu => {
            // Get process CPU time (stub - would track per-process)
            let cycles = crate::time::read_tsc();
            let ns = crate::time::tsc::cycles_to_ns(cycles);
            Ok(TimeSpec::from_nanos(ns as i64))
        }
        ClockId::ThreadCpu => {
            // Get thread CPU time (stub - would track per-thread)
            let cycles = crate::time::read_tsc();
            let ns = crate::time::tsc::cycles_to_ns(cycles);
            Ok(TimeSpec::from_nanos(ns as i64))
        }
    }
}

/// Set clock time (requires capability)
pub fn sys_clock_settime(clock_id: ClockId, time: TimeSpec) -> MemoryResult<()> {
    log::debug!("sys_clock_settime: clock_id={:?}, time={:?}", clock_id, time);
    
    // 1. Check capability (stub - would check CAP_SYS_TIME)
    
    // 2. Validate clock ID (only REALTIME can be set)
    match clock_id {
        ClockId::Realtime => {
            // 3. Update system time
            // Store boot time adjustment
            let uptime_secs = crate::time::uptime_ns() / 1_000_000_000;
            let new_boot_time = time.seconds as u64 - uptime_secs;
            
            // Update BOOT_TIME in time module (stub - needs access)
            log::info!("clock_settime: setting REALTIME to {} seconds", time.seconds);
            Ok(())
        }
        _ => {
            log::warn!("clock_settime: cannot set {:?}", clock_id);
            Err(MemoryError::PermissionDenied)
        }
    }
}

/// Get clock resolution
pub fn sys_clock_getres(clock_id: ClockId) -> MemoryResult<TimeSpec> {
    log::debug!("sys_clock_getres: clock_id={:?}", clock_id);
    
    match clock_id {
        ClockId::Realtime | ClockId::Monotonic | ClockId::Boottime => {
            // TSC resolution: typically 1ns
            Ok(TimeSpec::new(0, 1))
        }
        ClockId::ProcessCpu | ClockId::ThreadCpu => {
            // CPU time tracking resolution: ~10ns
            Ok(TimeSpec::new(0, 10))
        }
    }
}

/// Sleep for specified time
pub fn sys_nanosleep(duration: TimeSpec) -> MemoryResult<()> {
    log::debug!("sys_nanosleep: duration={:?}", duration);
    
    // 1. Validate duration
    if duration.seconds < 0 || duration.nanoseconds < 0 {
        return Err(MemoryError::InvalidParameter);
    }
    
    // 2. Convert to nanoseconds
    let total_ns = duration.as_nanos() as u64;
    
    // 3. Use busy sleep for now (proper implementation would block thread)
    if total_ns > 0 {
        crate::time::busy_sleep_ns(total_ns);
    }
    
    Ok(())
}

const TIMER_ABSTIME: u32 = 1;

/// Sleep until absolute time
pub fn sys_clock_nanosleep(clock_id: ClockId, flags: u32, time: TimeSpec) -> MemoryResult<()> {
    log::debug!("sys_clock_nanosleep: clock_id={:?}, flags={}, time={:?}", clock_id, flags, time);
    
    let duration = if flags & TIMER_ABSTIME != 0 {
        // 1. Absolute time - calculate delta
        let now = sys_clock_gettime(clock_id)?;
        let now_ns = now.as_nanos();
        let target_ns = time.as_nanos();
        
        if target_ns <= now_ns {
            // Already past target time
            return Ok(());
        }
        
        TimeSpec::from_nanos(target_ns - now_ns)
    } else {
        // Relative time
        time
    };
    
    // 2. Sleep for duration
    sys_nanosleep(duration)
}

use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone)]
struct Timer {
    id: TimerId,
    clock_id: ClockId,
    expiration: TimeSpec,
    interval: TimeSpec,
    armed: bool,
}

static TIMERS: Mutex<BTreeMap<TimerId, Timer>> = Mutex::new(BTreeMap::new());
static NEXT_TIMER_ID: AtomicU64 = AtomicU64::new(1);

/// Create timer
pub fn sys_timer_create(clock_id: ClockId, flags: u32) -> MemoryResult<TimerId> {
    log::debug!("sys_timer_create: clock_id={:?}, flags={}", clock_id, flags);
    
    // 1. Allocate timer ID
    let timer_id = NEXT_TIMER_ID.fetch_add(1, Ordering::SeqCst);
    
    // 2. Create timer structure
    let timer = Timer {
        id: timer_id,
        clock_id,
        expiration: TimeSpec::zero(),
        interval: TimeSpec::zero(),
        armed: false,
    };
    
    // 3. Register with timer subsystem
    let mut timers = TIMERS.lock();
    timers.insert(timer_id, timer);
    
    log::info!("timer_create: created timer {} for {:?}", timer_id, clock_id);
    Ok(timer_id)
}

/// Set timer
pub fn sys_timer_settime(
    timer_id: TimerId,
    flags: u32,
    value: TimeSpec,
    interval: TimeSpec,
) -> MemoryResult<()> {
    log::debug!(
        "sys_timer_settime: timer_id={}, flags={}, value={:?}, interval={:?}",
        timer_id, flags, value, interval
    );
    
    // 1. Find timer
    let mut timers = TIMERS.lock();
    let timer = timers.get_mut(&timer_id)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Set expiration time
    if flags & TIMER_ABSTIME != 0 {
        // Absolute time
        timer.expiration = value;
    } else {
        // Relative time - add to current time
        let now = sys_clock_gettime(timer.clock_id)?;
        let now_ns = now.as_nanos();
        let value_ns = value.as_nanos();
        timer.expiration = TimeSpec::from_nanos(now_ns + value_ns);
    }
    
    // 3. Set interval for periodic timers
    timer.interval = interval;
    
    // 4. Arm timer
    timer.armed = value.as_nanos() > 0;
    
    log::info!("timer_settime: timer {} armed, expires at {:?}",
        timer_id, timer.expiration);
    
    Ok(())
}

/// Get timer
pub fn sys_timer_gettime(timer_id: TimerId) -> MemoryResult<(TimeSpec, TimeSpec)> {
    log::debug!("sys_timer_gettime: timer_id={}", timer_id);
    
    // 1. Find timer
    let timers = TIMERS.lock();
    let timer = timers.get(&timer_id)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Calculate remaining time
    let remaining = if timer.armed {
        let now = sys_clock_gettime(timer.clock_id)?;
        let now_ns = now.as_nanos();
        let exp_ns = timer.expiration.as_nanos();
        
        if exp_ns > now_ns {
            TimeSpec::from_nanos(exp_ns - now_ns)
        } else {
            TimeSpec::zero()
        }
    } else {
        TimeSpec::zero()
    };
    
    Ok((remaining, timer.interval))
}

/// Delete timer
pub fn sys_timer_delete(timer_id: TimerId) -> MemoryResult<()> {
    log::debug!("sys_timer_delete: timer_id={}", timer_id);
    
    // 1. Find and remove timer
    let mut timers = TIMERS.lock();
    let timer = timers.remove(&timer_id)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Disarm timer (already done by removal)
    log::info!("timer_delete: deleted timer {}", timer_id);
    
    Ok(())
}

/// Get time of day (deprecated, use clock_gettime)
pub fn sys_gettimeofday() -> MemoryResult<TimeSpec> {
    sys_clock_gettime(ClockId::Realtime)
}

/// Set time of day (deprecated, use clock_settime)
pub fn sys_settimeofday(time: TimeSpec) -> MemoryResult<()> {
    sys_clock_settime(ClockId::Realtime, time)
}

/// Get uptime
pub fn sys_uptime() -> MemoryResult<TimeSpec> {
    sys_clock_gettime(ClockId::Boottime)
}

static ALARM_TIMER: Mutex<Option<(TimerId, TimeSpec)>> = Mutex::new(None);

/// Alarm - set timer signal
pub fn sys_alarm(seconds: u64) -> MemoryResult<u64> {
    log::debug!("sys_alarm: seconds={}", seconds);
    
    let mut alarm = ALARM_TIMER.lock();
    
    // 1. Get remaining time of previous alarm
    let remaining = if let Some((timer_id, expiration)) = *alarm {
        let now = sys_clock_gettime(ClockId::Realtime)?;
        let now_ns = now.as_nanos();
        let exp_ns = expiration.as_nanos();
        
        if exp_ns > now_ns {
            ((exp_ns - now_ns) / 1_000_000_000) as u64
        } else {
            0
        }
    } else {
        0
    };
    
    // 2. Cancel previous alarm
    if let Some((timer_id, _)) = alarm.take() {
        let _ = sys_timer_delete(timer_id);
    }
    
    // 3. Set new alarm timer if seconds > 0
    if seconds > 0 {
        let timer_id = sys_timer_create(ClockId::Realtime, 0)?;
        let duration = TimeSpec::new(seconds as i64, 0);
        
        sys_timer_settime(timer_id, 0, duration, TimeSpec::zero())?;
        
        let now = sys_clock_gettime(ClockId::Realtime)?;
        let expiration = TimeSpec::from_nanos(now.as_nanos() + duration.as_nanos());
        
        *alarm = Some((timer_id, expiration));
        
        log::info!("alarm: set for {} seconds", seconds);
    }
    
    Ok(remaining)
}
