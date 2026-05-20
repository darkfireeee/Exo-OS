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
pub mod report;
pub mod timeline;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use memory_dump::{
    crc32, delete_dump, enumerate_dumps, get_dump_stats, memory_dump_init, retrieve_dump,
    store_dump, verify_all_dumps, verify_dump_checksum, DumpChecksum, DumpRegion, DumpStats,
    DumpStorage,
};

pub use timeline::{
    correlate_events, get_correlation_chain, get_timeline_stats, query_timeline,
    query_timeline_by_time, query_timeline_by_type, record_timeline_event, timeline_init,
    TimelineCorrelation, TimelineEntry, TimelineEventType, TimelineStats,
};

pub use report::{
    deserialize_report, generate_report, get_report_stats, report_init, serialize_report,
    IncidentDetail, Recommendation, Report, ReportStats, ThreatCategory, ThreatLevel,
    ThreatSummary,
};
