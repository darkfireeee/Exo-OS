//! ProcFS - Process Information Filesystem
//!
//! ## Structure
//! ```
//! /proc/
//!   ├── cpuinfo          - CPU information
//!   ├── meminfo          - Memory statistics
//!   ├── stat             - Kernel/system statistics
//!   ├── uptime           - System uptime
//!   ├── loadavg          - Load average
//!   ├── version          - Kernel version
//!   ├── cmdline          - Kernel command line
//!   ├── mounts           - Mount information
//!   ├── filesystems      - Supported filesystems
//!   ├── devices          - Device list
//!   ├── interrupts       - Interrupt statistics
//!   ├── [pid]/           - Per-process directories
//!   │   ├── status       - Process status
//!   │   ├── stat         - Process statistics
//!   │   ├── cmdline      - Command line arguments
//!   │   ├── environ      - Environment variables
//!   │   ├── maps         - Memory mappings
//!   │   ├── fd/          - File descriptors (symlinks)
//!   │   ├── cwd -> path  - Current working directory
//!   │   └── exe -> path  - Executable path
//!   └── self -> [pid]    - Symlink to current process
//! ```
//!
//! ## Features
//! - Dynamic content generation
//! - Real-time statistics
//! - Zero memory overhead for unused entries
//! - POSIX-compliant format

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;
use hashbrown::HashMap;

use crate::fs::core::types::{
    Inode, InodeType, InodePermissions, Timestamp,
};
use crate::fs::{FsError, FsResult};

/// ProcFS entry types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProcEntry {
    /// Root directory
    Root,

    /// Global files
    CpuInfo,
    MemInfo,
    Stat,
    Uptime,
    LoadAvg,
    Version,
    Cmdline,
    Mounts,
    Filesystems,
    Devices,
    Interrupts,

    /// Per-process entries
    ProcessDir(u64),
    ProcessStatus(u64),
    ProcessStat(u64),
    ProcessCmdline(u64),
    ProcessEnviron(u64),
    ProcessMaps(u64),
    ProcessFdDir(u64),
    ProcessFd(u64, u32),
    ProcessCwd(u64),
    ProcessExe(u64),
}

/// Process information (stub - will be replaced with real process manager integration)
#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: u64,
    pub name: String,
    pub state: char, // R, S, D, Z, T
    pub ppid: u64,
    pub uid: u32,
    pub gid: u32,
    pub vm_size: u64,
    pub vm_rss: u64,
    pub threads: u32,
}

impl ProcessInfo {
    fn stub(pid: u64) -> Self {
        Self {
            pid,
            name: format!("init"),
            state: 'R',
            ppid: 0,
            uid: 0,
            gid: 0,
            vm_size: 1024 * 1024,
            vm_rss: 512 * 1024,
            threads: 1,
        }
    }
}

/// Generate /proc/cpuinfo
fn generate_cpuinfo() -> Vec<u8> {
    let info = format!(
        "processor\t: 0\n\
         vendor_id\t: GenuineIntel\n\
         cpu family\t: 6\n\
         model\t\t: 165\n\
         model name\t: Exo-OS Virtual CPU\n\
         stepping\t: 5\n\
         microcode\t: 0x0\n\
         cpu MHz\t\t: 3600.000\n\
         cache size\t: 8192 KB\n\
         physical id\t: 0\n\
         siblings\t: 1\n\
         core id\t\t: 0\n\
         cpu cores\t: 1\n\
         apicid\t\t: 0\n\
         initial apicid\t: 0\n\
         fpu\t\t: yes\n\
         fpu_exception\t: yes\n\
         cpuid level\t: 22\n\
         wp\t\t: yes\n\
         flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36\n\
         bugs\t\t:\n\
         bogomips\t: 7200.00\n\
         clflush size\t: 64\n\
         cache_alignment\t: 64\n\
         address sizes\t: 46 bits physical, 48 bits virtual\n"
    );
    info.into_bytes()
}

/// Generate /proc/meminfo
fn generate_meminfo() -> Vec<u8> {
    // Stub implementation - would query actual memory manager
    let total = 128 * 1024 * 1024; // 128 MB
    let used = 64 * 1024 * 1024;   // 64 MB
    let free = total - used;

    let info = format!(
        "MemTotal:      {} kB\n\
         MemFree:       {} kB\n\
         MemAvailable:  {} kB\n\
         Buffers:       0 kB\n\
         Cached:        0 kB\n\
         SwapCached:    0 kB\n\
         Active:        {} kB\n\
         Inactive:      0 kB\n\
         SwapTotal:     0 kB\n\
         SwapFree:      0 kB\n\
         Dirty:         0 kB\n\
         Writeback:     0 kB\n\
         Mapped:        0 kB\n\
         Slab:          0 kB\n\
         SReclaimable:  0 kB\n\
         SUnreclaim:    0 kB\n\
         PageTables:    0 kB\n\
         Committed_AS:  {} kB\n",
        total / 1024,
        free / 1024,
        free / 1024,
        used / 1024,
        used / 1024,
    );
    info.into_bytes()
}

/// Generate /proc/stat
fn generate_stat() -> Vec<u8> {
    let uptime_ticks = crate::time::uptime_ns() / 10_000_000; // Convert to centiseconds

    let info = format!(
        "cpu  {} 0 {} 0 0 0 0 0 0 0\n\
         cpu0 {} 0 {} 0 0 0 0 0 0 0\n\
         intr 0\n\
         ctxt 0\n\
         btime {}\n\
         processes 1\n\
         procs_running 1\n\
         procs_blocked 0\n",
        uptime_ticks / 2, uptime_ticks / 2,
        uptime_ticks / 2, uptime_ticks / 2,
        crate::time::unix_timestamp(),
    );
    info.into_bytes()
}

/// Generate /proc/uptime
fn generate_uptime() -> Vec<u8> {
    let uptime_sec = crate::time::uptime_ns() / 1_000_000_000;
    let idle_sec = uptime_sec; // Simple approximation

    let info = format!("{}.00 {}.00\n", uptime_sec, idle_sec);
    info.into_bytes()
}

/// Generate /proc/loadavg
fn generate_loadavg() -> Vec<u8> {
    // Stub - real load average calculation would go here
    let info = format!("0.00 0.00 0.00 1/1 1\n");
    info.into_bytes()
}

/// Generate /proc/version
fn generate_version() -> Vec<u8> {
    let info = format!(
        "Exo-OS version 0.7.0 (exo-kernel) #1 SMP {}\n",
        "Mon Jan 1 00:00:00 UTC 2025"
    );
    info.into_bytes()
}

/// Generate /proc/cmdline
fn generate_cmdline() -> Vec<u8> {
    // Stub - would read from bootloader
    let info = "quiet splash\n";
    info.as_bytes().to_vec()
}

/// Generate /proc/mounts
fn generate_mounts() -> Vec<u8> {
    let info = "tmpfs / tmpfs rw,relatime 0 0\n\
                devfs /dev devfs rw,relatime 0 0\n\
                procfs /proc procfs ro,relatime 0 0\n\
                sysfs /sys sysfs ro,relatime 0 0\n";
    info.as_bytes().to_vec()
}

/// Generate /proc/filesystems
fn generate_filesystems() -> Vec<u8> {
    let info = "nodev\tdevfs\n\
                nodev\tprocfs\n\
                nodev\tsysfs\n\
                nodev\ttmpfs\n\
                nodev\tpipefs\n\
                nodev\tsocketfs\n\
                \text4plus\n\
                \tfat32\n";
    info.as_bytes().to_vec()
}

/// Generate /proc/devices
fn generate_devices() -> Vec<u8> {
    let info = "Character devices:\n\
                  1 mem\n\
                  4 tty\n\
                  5 console\n\
                \n\
                Block devices:\n\
                  1 ramdisk\n";
    info.as_bytes().to_vec()
}

/// Generate /proc/interrupts
fn generate_interrupts() -> Vec<u8> {
    let info = "           CPU0\n\
                  0:          0   IO-APIC   2-edge      timer\n\
                  1:          0   IO-APIC   1-edge      i8042\n";
    info.as_bytes().to_vec()
}

/// Generate /proc/[pid]/status
fn generate_process_status(pid: u64) -> Vec<u8> {
    let proc = ProcessInfo::stub(pid);

    let info = format!(
        "Name:\t{}\n\
         State:\t{} (running)\n\
         Tgid:\t{}\n\
         Pid:\t{}\n\
         PPid:\t{}\n\
         Uid:\t{}\t{}\t{}\t{}\n\
         Gid:\t{}\t{}\t{}\t{}\n\
         VmSize:\t{} kB\n\
         VmRSS:\t{} kB\n\
         Threads:\t{}\n",
        proc.name,
        proc.state,
        proc.pid,
        proc.pid,
        proc.ppid,
        proc.uid, proc.uid, proc.uid, proc.uid,
        proc.gid, proc.gid, proc.gid, proc.gid,
        proc.vm_size / 1024,
        proc.vm_rss / 1024,
        proc.threads,
    );
    info.into_bytes()
}

/// Generate /proc/[pid]/stat
fn generate_process_stat(pid: u64) -> Vec<u8> {
    let proc = ProcessInfo::stub(pid);

    let info = format!(
        "{} ({}) {} {} 0 0 0 0 0 0 0 0 0 0 0 0 {} 0 0 0 0 0 0 {} {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        proc.pid,
        proc.name,
        proc.state,
        proc.ppid,
        proc.threads,
        proc.vm_size,
        proc.vm_rss / 4096, // RSS in pages
    );
    info.into_bytes()
}

/// Generate /proc/[pid]/cmdline
fn generate_process_cmdline(pid: u64) -> Vec<u8> {
    let proc = ProcessInfo::stub(pid);
    format!("{}\0", proc.name).into_bytes()
}

/// Generate /proc/[pid]/environ
fn generate_process_environ(_pid: u64) -> Vec<u8> {
    "PATH=/bin:/usr/bin\0HOME=/root\0".as_bytes().to_vec()
}

/// Generate /proc/[pid]/maps
fn generate_process_maps(_pid: u64) -> Vec<u8> {
    let info = "00400000-00500000 r-xp 00000000 00:00 0          [text]\n\
                00600000-00700000 rw-p 00000000 00:00 0          [data]\n\
                00800000-00900000 rw-p 00000000 00:00 0          [heap]\n\
                7fffffff000-7ffffffffff rw-p 00000000 00:00 0   [stack]\n";
    info.as_bytes().to_vec()
}

/// ProcFS Inode
pub struct ProcInode {
    ino: u64,
    entry: ProcEntry,
    content: RwLock<Option<Vec<u8>>>,
}

impl ProcInode {
    fn new(ino: u64, entry: ProcEntry) -> Self {
        Self {
            ino,
            entry,
            content: RwLock::new(None),
        }
    }

    /// Generate content on-demand
    fn generate_content(&self) -> Vec<u8> {
        match &self.entry {
            ProcEntry::CpuInfo => generate_cpuinfo(),
            ProcEntry::MemInfo => generate_meminfo(),
            ProcEntry::Stat => generate_stat(),
            ProcEntry::Uptime => generate_uptime(),
            ProcEntry::LoadAvg => generate_loadavg(),
            ProcEntry::Version => generate_version(),
            ProcEntry::Cmdline => generate_cmdline(),
            ProcEntry::Mounts => generate_mounts(),
            ProcEntry::Filesystems => generate_filesystems(),
            ProcEntry::Devices => generate_devices(),
            ProcEntry::Interrupts => generate_interrupts(),
            ProcEntry::ProcessStatus(pid) => generate_process_status(*pid),
            ProcEntry::ProcessStat(pid) => generate_process_stat(*pid),
            ProcEntry::ProcessCmdline(pid) => generate_process_cmdline(*pid),
            ProcEntry::ProcessEnviron(pid) => generate_process_environ(*pid),
            ProcEntry::ProcessMaps(pid) => generate_process_maps(*pid),
            _ => Vec::new(),
        }
    }

    fn is_directory(&self) -> bool {
        matches!(
            self.entry,
            ProcEntry::Root | ProcEntry::ProcessDir(_) | ProcEntry::ProcessFdDir(_)
        )
    }

    fn list_directory(&self) -> Vec<String> {
        match &self.entry {
            ProcEntry::Root => {
                let mut entries = vec![
                    "cpuinfo".to_string(),
                    "meminfo".to_string(),
                    "stat".to_string(),
                    "uptime".to_string(),
                    "loadavg".to_string(),
                    "version".to_string(),
                    "cmdline".to_string(),
                    "mounts".to_string(),
                    "filesystems".to_string(),
                    "devices".to_string(),
                    "interrupts".to_string(),
                    "1".to_string(), // init process
                    "self".to_string(),
                ];
                entries.sort();
                entries
            }
            ProcEntry::ProcessDir(_) => {
                vec![
                    "status".to_string(),
                    "stat".to_string(),
                    "cmdline".to_string(),
                    "environ".to_string(),
                    "maps".to_string(),
                    "fd".to_string(),
                    "cwd".to_string(),
                    "exe".to_string(),
                ]
            }
            ProcEntry::ProcessFdDir(_) => {
                // Stub - would list actual file descriptors
                vec!["0".to_string(), "1".to_string(), "2".to_string()]
            }
            _ => Vec::new(),
        }
    }
}

impl Inode for ProcInode {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn inode_type(&self) -> InodeType {
        if self.is_directory() {
            InodeType::Directory
        } else if matches!(self.entry, ProcEntry::ProcessCwd(_) | ProcEntry::ProcessExe(_) | ProcEntry::ProcessFd(_, _)) {
            InodeType::Symlink
        } else {
            InodeType::File
        }
    }

    fn size(&self) -> u64 {
        if self.is_directory() {
            0
        } else {
            self.generate_content().len() as u64
        }
    }

    fn permissions(&self) -> InodePermissions {
        if self.is_directory() {
            InodePermissions::from_octal(0o555)
        } else {
            InodePermissions::from_octal(0o444)
        }
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if self.is_directory() {
            return Err(FsError::IsDirectory);
        }

        let content = self.generate_content();
        let offset = offset as usize;

        if offset >= content.len() {
            return Ok(0);
        }

        let to_read = buf.len().min(content.len() - offset);
        buf[..to_read].copy_from_slice(&content[offset..offset + to_read]);

        Ok(to_read)
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::PermissionDenied)
    }

    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        Err(FsError::PermissionDenied)
    }

    fn list(&self) -> FsResult<Vec<String>> {
        if !self.is_directory() {
            return Err(FsError::NotDirectory);
        }

        Ok(self.list_directory())
    }

    fn lookup(&self, name: &str) -> FsResult<u64> {
        if !self.is_directory() {
            return Err(FsError::NotDirectory);
        }

        let entries = self.list_directory();
        if entries.contains(&name.to_string()) {
            // Return a pseudo-inode number
            // In a real implementation, this would integrate with the VFS
            Ok(self.ino + name.len() as u64)
        } else {
            Err(FsError::NotFound)
        }
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::PermissionDenied)
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::PermissionDenied)
    }

    fn readlink(&self) -> FsResult<String> {
        match &self.entry {
            ProcEntry::ProcessCwd(_) => Ok("/".to_string()),
            ProcEntry::ProcessExe(_) => Ok("/bin/init".to_string()),
            ProcEntry::ProcessFd(_, fd) => Ok(format!("/dev/pts/{}", fd)),
            _ => Err(FsError::InvalidArgument),
        }
    }
}

/// ProcFS - Process Information Filesystem
pub struct ProcFs {
    next_ino: AtomicU64,
    entries: RwLock<HashMap<String, Arc<ProcInode>>>,
}

impl ProcFs {
    pub fn new() -> Self {
        Self {
            next_ino: AtomicU64::new(10000),
            entries: RwLock::new(HashMap::new()),
        }
    }

    fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::Relaxed)
    }

    /// Get or create an inode for a procfs entry
    pub fn get_inode(&self, path: &str) -> FsResult<Arc<ProcInode>> {
        let entry = self.parse_path(path)?;

        let mut entries = self.entries.write();

        if let Some(inode) = entries.get(path) {
            return Ok(inode.clone());
        }

        let ino = self.alloc_ino();
        let inode = Arc::new(ProcInode::new(ino, entry));
        entries.insert(path.to_string(), inode.clone());

        Ok(inode)
    }

    /// Parse a path to determine the ProcEntry type
    fn parse_path(&self, path: &str) -> FsResult<ProcEntry> {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return Ok(ProcEntry::Root);
        }

        match parts[0] {
            "cpuinfo" => Ok(ProcEntry::CpuInfo),
            "meminfo" => Ok(ProcEntry::MemInfo),
            "stat" => Ok(ProcEntry::Stat),
            "uptime" => Ok(ProcEntry::Uptime),
            "loadavg" => Ok(ProcEntry::LoadAvg),
            "version" => Ok(ProcEntry::Version),
            "cmdline" => Ok(ProcEntry::Cmdline),
            "mounts" => Ok(ProcEntry::Mounts),
            "filesystems" => Ok(ProcEntry::Filesystems),
            "devices" => Ok(ProcEntry::Devices),
            "interrupts" => Ok(ProcEntry::Interrupts),
            pid_str => {
                if let Ok(pid) = pid_str.parse::<u64>() {
                    if parts.len() == 1 {
                        return Ok(ProcEntry::ProcessDir(pid));
                    }

                    match parts[1] {
                        "status" => Ok(ProcEntry::ProcessStatus(pid)),
                        "stat" => Ok(ProcEntry::ProcessStat(pid)),
                        "cmdline" => Ok(ProcEntry::ProcessCmdline(pid)),
                        "environ" => Ok(ProcEntry::ProcessEnviron(pid)),
                        "maps" => Ok(ProcEntry::ProcessMaps(pid)),
                        "fd" => {
                            if parts.len() == 2 {
                                Ok(ProcEntry::ProcessFdDir(pid))
                            } else if let Ok(fd) = parts[2].parse::<u32>() {
                                Ok(ProcEntry::ProcessFd(pid, fd))
                            } else {
                                Err(FsError::NotFound)
                            }
                        }
                        "cwd" => Ok(ProcEntry::ProcessCwd(pid)),
                        "exe" => Ok(ProcEntry::ProcessExe(pid)),
                        _ => Err(FsError::NotFound),
                    }
                } else {
                    Err(FsError::NotFound)
                }
            }
        }
    }
}

/// Global ProcFS instance
static PROCFS: spin::Once<ProcFs> = spin::Once::new();

/// Initialize ProcFS
pub fn init() {
    PROCFS.call_once(|| ProcFs::new());
}

/// Get global ProcFS instance
pub fn get() -> &'static ProcFs {
    PROCFS.get().expect("ProcFS not initialized")
}
