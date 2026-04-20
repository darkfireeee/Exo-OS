//! # forensics — Post-incident forensic analysis
//!
//! Provides memory dump, timeline reconstruction, and report generation
//! capabilities for post-incident forensic analysis.
//!
//! ## Modules
//! - `memory_dump` — Memory region dump storage & checksum verification
//! - `timeline`    — Timeline reconstruction & event correlation
//! - `report`      — Forensic report generation & serialization

pub mod memory_dump;
pub mod timeline;
pub mod report;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use memory_dump::{
    DumpRegion, DumpStorage, DumpChecksum, DumpStats,
    store_dump, retrieve_dump, verify_dump_checksum,
    enumerate_dumps, delete_dump, verify_all_dumps,
    crc32, get_dump_stats, memory_dump_init,
};

pub use timeline::{
    TimelineEntry, TimelineEventType, TimelineCorrelation, TimelineStats,
    record_timeline_event, query_timeline, query_timeline_by_time,
    query_timeline_by_type, correlate_events, get_correlation_chain,
    get_timeline_stats, timeline_init,
};

pub use report::{
    Report, ThreatSummary, IncidentDetail, Recommendation,
    ThreatLevel, ThreatCategory, ReportStats,
    generate_report, serialize_report, deserialize_report,
    get_report_stats, report_init,
};
