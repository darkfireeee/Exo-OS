//! ProcFS - Process Information Filesystem
//! 
//! Expose kernel and process information as files.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// ProcFS entry
pub enum ProcEntry {
    /// /proc/cpuinfo
    CpuInfo,
    /// /proc/meminfo
    MemInfo,
    /// /proc/uptime
    Uptime,
    /// /proc/version
    Version,
    /// /proc/[pid]/status
    ProcessStatus(u64),
    /// /proc/[pid]/cmdline
    ProcessCmdline(u64),
}

/// Read proc entry
pub fn read_entry(entry: ProcEntry) -> Result<Vec<u8>, &'static str> {
    match entry {
        ProcEntry::CpuInfo => {
            let info = format!(
                "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Exo-OS CPU\n"
            );
            Ok(info.into_bytes())
        }
        ProcEntry::MemInfo => {
            let info = format!(
                "MemTotal:\t{}\nMemFree:\t{}\n",
                "TODO", "TODO"
            );
            Ok(info.into_bytes())
        }
        ProcEntry::Uptime => {
            // TODO: Get actual uptime
            Ok(b"0.00 0.00\n".to_vec())
        }
        ProcEntry::Version => {
            let version = format!("Exo-OS version 0.1.0\n");
            Ok(version.into_bytes())
        }
        ProcEntry::ProcessStatus(pid) => {
            let status = format!("Name:\tProcess{}\nPid:\t{}\n", pid, pid);
            Ok(status.into_bytes())
        }
        ProcEntry::ProcessCmdline(pid) => {
            // TODO: Get actual cmdline
            Ok(format!("process{}\0", pid).into_bytes())
        }
    }
}
