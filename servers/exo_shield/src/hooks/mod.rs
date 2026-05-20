//! # hooks — Exo-Shield hook subsystem
//!
//! Provides interception hooks for execution, network, memory, and syscall
//! events. Every hook runs in the exo_shield server context (Ring 1) and
//! feeds the detection engine with structured events.
//!
//! ## Modules
//! - `exec_hooks`   — process execution interception & chain detection
//! - `net_hooks`    — connection monitoring, port-scan & exfiltration detection
//! - `memory_hooks` — allocation monitoring, overflow & UAF detection
//! - `syscall_hooks`— syscall frequency & dangerous-syscall detection

pub mod exec_hooks;
pub mod memory_hooks;
pub mod net_hooks;
pub mod syscall_hooks;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use exec_hooks::{
    add_blacklist_path, check_exec_chain, exec_hooks_init, get_exec_chain_for_pid, get_exec_stats,
    post_exec_monitor, pre_exec_validate, query_exec_events_for_pid, record_exec_event,
    remove_blacklist_path, ExecAction, ExecChainEntry, ExecEvent, ExecStats,
};

pub use net_hooks::{
    close_connection, detect_exfiltration, detect_port_scan, get_net_stats, net_hooks_init,
    post_connect_monitor, pre_connect_check, query_dns_for_pid, record_dns_query, DnsQueryEntry,
    NetEvent, NetEventType, NetStats,
};

pub use memory_hooks::{
    detect_buffer_overflow, detect_use_after_free, get_mem_stats, mem_hooks_init,
    post_alloc_monitor, pre_alloc_check, query_mem_events_for_pid, record_free, scan_memory_region,
    verify_canaries_for_pid, AllocRecord, FreedRegion, MemEvent, MemEventType, MemStats,
};

pub use syscall_hooks::{
    analyze_syscall_sequence, detect_dangerous_syscall, get_syscall_freq, get_syscall_sequence,
    get_syscall_stats, post_syscall_monitor, pre_syscall_check, query_syscall_events_for_pid,
    syscall_hooks_init, SyscallEvent, SyscallFreqEntry, SyscallSeqEntry, SyscallStats,
};
