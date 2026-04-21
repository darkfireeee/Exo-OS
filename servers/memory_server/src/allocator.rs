use exo_syscall_abi as syscall;

const MAX_QUOTA_ENTRIES: usize = 64;
const DEFAULT_LIMIT_BYTES: u64 = u64::MAX;

#[derive(Clone, Copy)]
struct QuotaEntry {
    active: bool,
    pid: u32,
    used_bytes: u64,
    peak_bytes: u64,
    limit_bytes: u64,
}

impl QuotaEntry {
    const fn empty() -> Self {
        Self {
            active: false,
            pid: 0,
            used_bytes: 0,
            peak_bytes: 0,
            limit_bytes: DEFAULT_LIMIT_BYTES,
        }
    }
}

#[derive(Clone, Copy)]
pub struct QuotaSnapshot {
    pub used_bytes: u64,
    pub peak_bytes: u64,
    pub limit_bytes: u64,
}

pub struct QuotaTable {
    entries: [QuotaEntry; MAX_QUOTA_ENTRIES],
}

impl QuotaTable {
    pub const fn new() -> Self {
        Self {
            entries: [QuotaEntry::empty(); MAX_QUOTA_ENTRIES],
        }
    }

    fn entry_mut(&mut self, pid: u32) -> Result<&mut QuotaEntry, i64> {
        if let Some(idx) = self.entries.iter().position(|entry| entry.active && entry.pid == pid) {
            return Ok(&mut self.entries[idx]);
        }

        let Some(idx) = self.entries.iter().position(|entry| !entry.active) else {
            return Err(syscall::ENOSPC);
        };

        self.entries[idx] = QuotaEntry {
            active: true,
            pid,
            used_bytes: 0,
            peak_bytes: 0,
            limit_bytes: DEFAULT_LIMIT_BYTES,
        };
        Ok(&mut self.entries[idx])
    }

    pub fn reserve(&mut self, pid: u32, bytes: u64) -> Result<QuotaSnapshot, i64> {
        let entry = self.entry_mut(pid)?;
        let next = entry.used_bytes.checked_add(bytes).ok_or(syscall::ENOMEM)?;
        if next > entry.limit_bytes {
            return Err(syscall::ENOMEM);
        }

        entry.used_bytes = next;
        if next > entry.peak_bytes {
            entry.peak_bytes = next;
        }

        Ok(QuotaSnapshot {
            used_bytes: entry.used_bytes,
            peak_bytes: entry.peak_bytes,
            limit_bytes: entry.limit_bytes,
        })
    }

    pub fn release(&mut self, pid: u32, bytes: u64) -> Result<QuotaSnapshot, i64> {
        let entry = self.entry_mut(pid)?;
        entry.used_bytes = entry.used_bytes.saturating_sub(bytes);
        Ok(QuotaSnapshot {
            used_bytes: entry.used_bytes,
            peak_bytes: entry.peak_bytes,
            limit_bytes: entry.limit_bytes,
        })
    }

    pub fn set_limit(&mut self, pid: u32, limit_bytes: u64) -> Result<QuotaSnapshot, i64> {
        let entry = self.entry_mut(pid)?;
        if limit_bytes < entry.used_bytes {
            return Err(syscall::EBUSY);
        }
        entry.limit_bytes = limit_bytes;
        Ok(QuotaSnapshot {
            used_bytes: entry.used_bytes,
            peak_bytes: entry.peak_bytes,
            limit_bytes: entry.limit_bytes,
        })
    }

    pub fn snapshot(&mut self, pid: u32) -> Result<QuotaSnapshot, i64> {
        let entry = self.entry_mut(pid)?;
        Ok(QuotaSnapshot {
            used_bytes: entry.used_bytes,
            peak_bytes: entry.peak_bytes,
            limit_bytes: entry.limit_bytes,
        })
    }
}
