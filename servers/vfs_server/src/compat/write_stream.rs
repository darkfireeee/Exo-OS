//! Write stream compatibility policy for database-style workloads.

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalPattern {
    Unknown = 0,
    SqliteWal = 1,
    SqliteJournal = 2,
    PostgresWal = 3,
    Custom = 4,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteHistory {
    pub avg_write_size: u32,
    pub sequential_ratio_pct: u8,
    pub fsync_ratio_pct: u8,
    pub _reserved: [u8; 2],
}

pub const fn coalescer_window_ms(pattern: JournalPattern) -> u32 {
    match pattern {
        JournalPattern::SqliteWal | JournalPattern::SqliteJournal => 0,
        JournalPattern::PostgresWal => 1,
        JournalPattern::Unknown | JournalPattern::Custom => 5,
    }
}

pub fn detect_pattern(name: &[u8], history: WriteHistory) -> JournalPattern {
    if ends_with(name, b".wal") {
        return JournalPattern::SqliteWal;
    }
    if ends_with(name, b"-journal") {
        return JournalPattern::SqliteJournal;
    }
    if ends_with(name, b".000000001") {
        return JournalPattern::PostgresWal;
    }
    if history.avg_write_size == 4096 && history.sequential_ratio_pct > 90 {
        return JournalPattern::SqliteWal;
    }
    if history.avg_write_size == 8192 && history.fsync_ratio_pct > 50 {
        return JournalPattern::PostgresWal;
    }
    JournalPattern::Unknown
}

fn ends_with(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && &haystack[haystack.len() - needle.len()..] == needle
}
