//! System V IPC Syscall Handlers
//!
//! Implements shmget, shmat, shmdt, shmctl, semget, semop, semctl, msgget, msgsnd, msgrcv, msgctl.

// Types
pub type Key = i32;
pub type ShmId = i32;
pub type SemId = i32;
pub type MsgId = i32;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IpcPerm {
    pub key: Key,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u16,
    pub seq: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ShmidDs {
    pub shm_perm: IpcPerm,
    pub shm_segsz: usize,
    pub shm_atime: i64,
    pub shm_dtime: i64,
    pub shm_ctime: i64,
    pub shm_cpid: i32,
    pub shm_lpid: i32,
    pub shm_nattch: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SemidDs {
    pub sem_perm: IpcPerm,
    pub sem_otime: i64,
    pub sem_ctime: i64,
    pub sem_nsems: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MsqidDs {
    pub msg_perm: IpcPerm,
    pub msg_stime: i64,
    pub msg_rtime: i64,
    pub msg_ctime: i64,
    pub msg_cbytes: usize,
    pub msg_qnum: usize,
    pub msg_qbytes: usize,
    pub msg_lspid: i32,
    pub msg_lrpid: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Sembuf {
    pub sem_num: u16,
    pub sem_op: i16,
    pub sem_flg: i16,
}

// Shared Memory
pub fn sys_shmget(key: Key, size: usize, shmflg: i32) -> i32 {
    log::info!(
        "sys_shmget: key={}, size={}, flags={:#x}",
        key,
        size,
        shmflg
    );
    // Stub: return a fake ID. In a real implementation, we would check existing keys or create new.
    1
}

pub fn sys_shmat(shmid: ShmId, shmaddr: *const u8, shmflg: i32) -> usize {
    log::info!(
        "sys_shmat: id={}, addr={:?}, flags={:#x}",
        shmid,
        shmaddr,
        shmflg
    );
    // Stub: return a fake address (e.g. 0x10000000)
    // In reality, we would map the shared memory segment into the process's address space.
    0x10000000
}

pub fn sys_shmdt(shmaddr: *const u8) -> i32 {
    log::info!("sys_shmdt: addr={:?}", shmaddr);
    0
}

pub fn sys_shmctl(shmid: ShmId, cmd: i32, _buf: *mut ShmidDs) -> i32 {
    log::info!("sys_shmctl: id={}, cmd={}", shmid, cmd);
    0
}

// Semaphores
pub fn sys_semget(key: Key, nsems: i32, semflg: i32) -> i32 {
    log::info!(
        "sys_semget: key={}, nsems={}, flags={:#x}",
        key,
        nsems,
        semflg
    );
    1
}

pub fn sys_semop(semid: SemId, _sops: *mut Sembuf, nsops: usize) -> i32 {
    log::info!("sys_semop: id={}, nsops={}", semid, nsops);
    0
}

pub fn sys_semctl(semid: SemId, semnum: i32, cmd: i32, _arg: usize) -> i32 {
    log::info!("sys_semctl: id={}, num={}, cmd={}", semid, semnum, cmd);
    0
}

// Message Queues
pub fn sys_msgget(key: Key, msgflg: i32) -> i32 {
    log::info!("sys_msgget: key={}, flags={:#x}", key, msgflg);
    1
}

pub fn sys_msgsnd(msqid: MsgId, _msgp: *const u8, msgsz: usize, msgflg: i32) -> i32 {
    log::info!(
        "sys_msgsnd: id={}, size={}, flags={:#x}",
        msqid,
        msgsz,
        msgflg
    );
    0
}

pub fn sys_msgrcv(msqid: MsgId, _msgp: *mut u8, msgsz: usize, msgtyp: isize, msgflg: i32) -> isize {
    log::info!(
        "sys_msgrcv: id={}, size={}, type={}, flags={:#x}",
        msqid,
        msgsz,
        msgtyp,
        msgflg
    );
    0
}

pub fn sys_msgctl(msqid: MsgId, cmd: i32, _buf: *mut MsqidDs) -> i32 {
    log::info!("sys_msgctl: id={}, cmd={}", msqid, cmd);
    0
}
