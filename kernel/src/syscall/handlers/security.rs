//! Security and Capability System Call Handlers
//!
//! Handles capability-based security operations

use crate::memory::{MemoryResult, MemoryError};

/// Capability identifier
pub type CapId = u64;

/// Capability types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityType {
    // File system capabilities
    FileRead = 0x01,
    FileWrite = 0x02,
    FileExecute = 0x04,
    FileCreate = 0x08,
    FileDelete = 0x10,
    
    // Process capabilities
    ProcessFork = 0x100,
    ProcessKill = 0x200,
    ProcessTrace = 0x400,
    
    // Memory capabilities
    MemoryMap = 0x1000,
    MemoryUnmap = 0x2000,
    MemoryProtect = 0x4000,
    
    // IPC capabilities
    IpcSend = 0x10000,
    IpcRecv = 0x20000,
    IpcCreate = 0x40000,
    
    // System capabilities
    SystemShutdown = 0x100000,
    SystemReboot = 0x200000,
    SystemTime = 0x400000,
    
    // Network capabilities
    NetBind = 0x1000000,
    NetConnect = 0x2000000,
    NetListen = 0x4000000,
}

/// Capability structure
#[derive(Debug, Clone)]
pub struct Capability {
    pub id: CapId,
    pub cap_type: CapabilityType,
    pub target: u64,  // PID, FD, or resource ID
    pub valid: bool,
}

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// Process capability tables (pid -> list of capabilities)
static PROCESS_CAPS: Mutex<BTreeMap<u64, Vec<Capability>>> = Mutex::new(BTreeMap::new());
static NEXT_CAP_ID: AtomicU64 = AtomicU64::new(1);

/// Check if process has capability
pub fn sys_check_capability(cap_type: CapabilityType, target: u64) -> MemoryResult<bool> {
    log::debug!("sys_check_capability: type={:?}, target={}", cap_type, target);
    
    // 1. Get current process
    let current_pid = crate::task::current().pid() as u64;
    
    // 2. Search capability list
    let caps = PROCESS_CAPS.lock();
    if let Some(cap_list) = caps.get(&current_pid) {
        // 3. Check if capability matches
        for cap in cap_list {
            if cap.valid && cap.cap_type == cap_type && 
               (cap.target == target || cap.target == 0) {
                return Ok(true);
            }
        }
    }
    
    log::debug!("check_capability: DENIED for PID {} {:?}", current_pid, cap_type);
    Ok(false)
}

/// Grant capability to process
pub fn sys_grant_capability(pid: u64, cap_type: CapabilityType, target: u64) -> MemoryResult<CapId> {
    log::debug!("sys_grant_capability: pid={}, type={:?}, target={}", pid, cap_type, target);
    
    // 1. Check if current process can grant (needs SystemAdmin or specific grant capability)
    let current_pid = crate::task::current().pid() as u64;
    if !has_grant_permission(current_pid) {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 2. Create capability
    let cap_id = NEXT_CAP_ID.fetch_add(1, Ordering::SeqCst);
    let capability = Capability {
        id: cap_id,
        cap_type,
        target,
        valid: true,
    };
    
    // 3. Add to target process
    let mut caps = PROCESS_CAPS.lock();
    caps.entry(pid)
        .or_insert_with(Vec::new)
        .push(capability);
    
    log::info!("grant_capability: PID {} granted {:?} for target {}", 
        pid, cap_type, target);
    Ok(cap_id)
}

fn has_grant_permission(pid: u64) -> bool {
    // Root process (PID 1) can always grant
    if pid == 1 {
        return true;
    }
    
    // Check for SystemShutdown capability (admin-level)
    let caps = PROCESS_CAPS.lock();
    if let Some(cap_list) = caps.get(&pid) {
        for cap in cap_list {
            if cap.valid && cap.cap_type == CapabilityType::SystemShutdown {
                return true;
            }
        }
    }
    
    false
}

/// Revoke capability from process
pub fn sys_revoke_capability(pid: u64, cap_id: CapId) -> MemoryResult<()> {
    log::debug!("sys_revoke_capability: pid={}, cap_id={}", pid, cap_id);
    
    // 1. Check permissions
    let current_pid = crate::task::current().pid() as u64;
    if !has_grant_permission(current_pid) && current_pid != pid {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 2. Find and mark capability as invalid
    let mut caps = PROCESS_CAPS.lock();
    if let Some(cap_list) = caps.get_mut(&pid) {
        for cap in cap_list.iter_mut() {
            if cap.id == cap_id {
                cap.valid = false;
                log::info!("revoke_capability: revoked cap {} from PID {}", 
                    cap_id, pid);
                return Ok(());
            }
        }
    }
    
    Err(MemoryError::NotFound)
}

/// Transfer capability to another process
pub fn sys_transfer_capability(target_pid: u64, cap_id: CapId) -> MemoryResult<()> {
    log::debug!("sys_transfer_capability: target_pid={}, cap_id={}", target_pid, cap_id);
    
    // TODO: Implement
    // 1. Check if capability is transferable
    // 2. Remove from current process
    // 3. Add to target process
    
    Ok(())
}

/// List capabilities of current process
pub fn sys_list_capabilities(buffer: &mut [Capability]) -> MemoryResult<usize> {
    log::debug!("sys_list_capabilities: buffer_len={}", buffer.len());
    
    // TODO: Implement
    // 1. Get current process
    // 2. Copy capabilities to buffer
    // 3. Return count
    
    Ok(0)
}

#[derive(Debug, Clone, Copy)]
struct ProcessCredentials {
    uid: u64,
    gid: u64,
    euid: u64,
    egid: u64,
}

static PROCESS_CREDS: Mutex<BTreeMap<u64, ProcessCredentials>> = Mutex::new(BTreeMap::new());

fn get_creds(pid: u64) -> ProcessCredentials {
    PROCESS_CREDS.lock()
        .get(&pid)
        .copied()
        .unwrap_or(ProcessCredentials {
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
        })
}

/// Set user ID (requires capability)
pub fn sys_setuid(uid: u64) -> MemoryResult<()> {
    log::debug!("sys_setuid: uid={}", uid);
    
    // 1. Check capability (root or CAP_SETUID)
    let current_pid = crate::task::current().pid() as u64;
    let creds = get_creds(current_pid);
    
    if creds.euid != 0 { // Not root
        return Err(MemoryError::PermissionDenied);
    }
    
    // 2. Update process UID
    let mut all_creds = PROCESS_CREDS.lock();
    let proc_creds = all_creds.entry(current_pid)
        .or_insert(creds);
    
    proc_creds.uid = uid;
    proc_creds.euid = uid;
    
    log::info!("setuid: PID {} set UID to {}", current_pid, uid);
    Ok(())
}

/// Set group ID (requires capability)
pub fn sys_setgid(gid: u64) -> MemoryResult<()> {
    log::debug!("sys_setgid: gid={}", gid);
    
    let current_pid = crate::task::current().pid() as u64;
    let creds = get_creds(current_pid);
    
    if creds.euid != 0 {
        return Err(MemoryError::PermissionDenied);
    }
    
    let mut all_creds = PROCESS_CREDS.lock();
    let proc_creds = all_creds.entry(current_pid)
        .or_insert(creds);
    
    proc_creds.gid = gid;
    proc_creds.egid = gid;
    
    log::info!("setgid: PID {} set GID to {}", current_pid, gid);
    Ok(())
}

/// Get user ID
pub fn sys_getuid() -> u64 {
    let current_pid = crate::task::current().pid() as u64;
    get_creds(current_pid).uid
}

/// Get group ID
pub fn sys_getgid() -> u64 {
    let current_pid = crate::task::current().pid() as u64;
    get_creds(current_pid).gid
}

/// Get effective user ID
pub fn sys_geteuid() -> u64 {
    let current_pid = crate::task::current().pid() as u64;
    get_creds(current_pid).euid
}

/// Get effective group ID
pub fn sys_getegid() -> u64 {
    let current_pid = crate::task::current().pid() as u64;
    get_creds(current_pid).egid
}

const SECCOMP_MODE_STRICT: u32 = 1;
const SECCOMP_MODE_FILTER: u32 = 2;

static SECCOMP_MODES: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new());

/// Secure computation mode (seccomp)
pub fn sys_seccomp(mode: u32, flags: u32, filter: usize) -> MemoryResult<()> {
    log::debug!("sys_seccomp: mode={}, flags={}, filter={:#x}", mode, flags, filter);
    
    // 1. Validate mode
    if mode != SECCOMP_MODE_STRICT && mode != SECCOMP_MODE_FILTER {
        return Err(MemoryError::InvalidParameter);
    }
    
    let current_pid = crate::task::current().pid() as u64;
    
    // 2. Install filter if provided (stub)
    if mode == SECCOMP_MODE_FILTER && filter != 0 {
        log::debug!("seccomp: filter mode with filter at {:#x}", filter);
        // Would install BPF filter here
    }
    
    // 3. Enable seccomp for process
    SECCOMP_MODES.lock().insert(current_pid, mode);
    
    log::info!("seccomp: PID {} enabled mode {}", current_pid, mode);
    Ok(())
}

#[derive(Debug, Clone)]
struct PledgePromises {
    stdio: bool,
    rpath: bool,
    wpath: bool,
    cpath: bool,
    inet: bool,
    unix: bool,
    proc: bool,
    exec: bool,
}

static PLEDGES: Mutex<BTreeMap<u64, PledgePromises>> = Mutex::new(BTreeMap::new());

/// Pledge - OpenBSD-style system call restrictions
pub fn sys_pledge(promises: &str) -> MemoryResult<()> {
    log::debug!("sys_pledge: promises={}", promises);
    
    // 1. Parse promises
    let mut pledge = PledgePromises {
        stdio: false,
        rpath: false,
        wpath: false,
        cpath: false,
        inet: false,
        unix: false,
        proc: false,
        exec: false,
    };
    
    for promise in promises.split_whitespace() {
        match promise {
            "stdio" => pledge.stdio = true,
            "rpath" => pledge.rpath = true,
            "wpath" => pledge.wpath = true,
            "cpath" => pledge.cpath = true,
            "inet" => pledge.inet = true,
            "unix" => pledge.unix = true,
            "proc" => pledge.proc = true,
            "exec" => pledge.exec = true,
            _ => log::warn!("pledge: unknown promise '{}'", promise),
        }
    }
    
    // 2-3. Install filter for process
    let current_pid = crate::task::current().pid() as u64;
    PLEDGES.lock().insert(current_pid, pledge);
    
    log::info!("pledge: PID {} restricted to '{}'", current_pid, promises);
    Ok(())
}

#[derive(Debug, Clone)]
struct UnveilEntry {
    path: alloc::string::String,
    read: bool,
    write: bool,
    execute: bool,
    create: bool,
}

static UNVEILS: Mutex<BTreeMap<u64, (Vec<UnveilEntry>, bool)>> = Mutex::new(BTreeMap::new());

/// Unveil - OpenBSD-style filesystem access restrictions
pub fn sys_unveil(path: &str, permissions: &str) -> MemoryResult<()> {
    log::debug!("sys_unveil: path={}, permissions={}", path, permissions);
    
    let current_pid = crate::task::current().pid() as u64;
    let mut unveils = UNVEILS.lock();
    let (list, locked) = unveils.entry(current_pid)
        .or_insert((Vec::new(), false));
    
    // 3. If path is empty, lock unveil list
    if path.is_empty() {
        *locked = true;
        log::info!("unveil: PID {} locked unveil list", current_pid);
        return Ok(());
    }
    
    // Check if already locked
    if *locked {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 1. Parse permissions
    let mut entry = UnveilEntry {
        path: alloc::string::String::from(path),
        read: false,
        write: false,
        execute: false,
        create: false,
    };
    
    for perm in permissions.chars() {
        match perm {
            'r' => entry.read = true,
            'w' => entry.write = true,
            'x' => entry.execute = true,
            'c' => entry.create = true,
            _ => {},
        }
    }
    
    // 2. Add to allowed list
    list.push(entry);
    
    log::info!("unveil: PID {} allowed '{}' with '{}'", 
        current_pid, path, permissions);
    Ok(())
}
