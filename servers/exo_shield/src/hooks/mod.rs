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
pub mod net_hooks;
pub mod memory_hooks;
pub mod syscall_hooks;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use exec_hooks::{
    ExecEvent, ExecAction, ExecChainEntry, ExecStats,
    pre_exec_validate, post_exec_monitor, check_exec_chain,
    record_exec_event, get_exec_stats, add_blacklist_path,
    remove_blacklist_path, get_exec_chain_for_pid,
    query_exec_events_for_pid, exec_hooks_init,
};

pub use net_hooks::{
    NetEvent, NetEventType, DnsQueryEntry, NetStats,
    pre_connect_check, post_connect_monitor, detect_port_scan,
    detect_exfiltration, record_dns_query, get_net_stats,
    query_dns_for_pid, close_connection, net_hooks_init,
};

pub use memory_hooks::{
    MemEvent, MemEventType, AllocRecord, FreedRegion, MemStats,
    pre_alloc_check, post_alloc_monitor, detect_buffer_overflow,
    detect_use_after_free, scan_memory_region, get_mem_stats,
    verify_canaries_for_pid, query_mem_events_for_pid,
    record_free, mem_hooks_init,
};

pub use syscall_hooks::{
    SyscallEvent, SyscallFreqEntry, SyscallSeqEntry, SyscallStats,
    pre_syscall_check, post_syscall_monitor, detect_dangerous_syscall,
    analyze_syscall_sequence, get_syscall_stats,
    get_syscall_freq, get_syscall_sequence,
    query_syscall_events_for_pid, syscall_hooks_init,
};
