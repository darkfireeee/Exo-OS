pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_RELAENT: i64 = 9;
pub const DT_STRSZ: i64 = 10;
pub const DT_SYMENT: i64 = 11;
pub const DT_INIT: i64 = 12;
pub const DT_FINI: i64 = 13;
pub const DT_SONAME: i64 = 14;
pub const DT_RPATH: i64 = 15;
pub const DT_REL: i64 = 17;
pub const DT_RELSZ: i64 = 18;
pub const DT_RELENT: i64 = 19;
pub const DT_PLTREL: i64 = 20;
pub const DT_JMPREL: i64 = 23;
pub const DT_BIND_NOW: i64 = 24;
pub const DT_INIT_ARRAY: i64 = 25;
pub const DT_FINI_ARRAY: i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
pub const DT_RUNPATH: i64 = 29;
pub const DT_FLAGS: i64 = 30;
pub const DT_RELA_COUNT: i64 = 0x6fff_fff9;
pub const DT_FLAGS_1: i64 = 0x6fff_fffb;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DynamicEntry {
    pub tag: i64,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DynamicInfo {
    pub needed_count: u16,
    pub strtab: u64,
    pub strsz: u64,
    pub symtab: u64,
    pub syment: u64,
    pub rela: u64,
    pub rela_size: u64,
    pub rela_entry_size: u64,
    pub rela_count_hint: u64,
    pub jmprel: u64,
    pub pltrel_size: u64,
    pub plt_rel_type: i64,
    pub init: u64,
    pub init_array: u64,
    pub init_array_size: u64,
    pub fini: u64,
    pub fini_array: u64,
    pub fini_array_size: u64,
    pub flags: u64,
    pub flags_1: u64,
    pub has_rel_table: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DynamicError {
    NullTable,
    InvalidEntrySize,
    UnsupportedRelTable,
}

impl DynamicInfo {
    pub fn record(&mut self, entry: DynamicEntry) -> Result<(), DynamicError> {
        match entry.tag {
            DT_NEEDED => {
                self.needed_count = self.needed_count.saturating_add(1);
            }
            DT_STRTAB => self.strtab = entry.value,
            DT_STRSZ => self.strsz = entry.value,
            DT_SYMTAB => self.symtab = entry.value,
            DT_SYMENT => self.syment = entry.value,
            DT_RELA => self.rela = entry.value,
            DT_RELASZ => self.rela_size = entry.value,
            DT_RELAENT => self.rela_entry_size = entry.value,
            DT_JMPREL => self.jmprel = entry.value,
            DT_PLTRELSZ => self.pltrel_size = entry.value,
            DT_PLTREL => self.plt_rel_type = entry.value as i64,
            DT_INIT => self.init = entry.value,
            DT_INIT_ARRAY => self.init_array = entry.value,
            DT_INIT_ARRAYSZ => self.init_array_size = entry.value,
            DT_FINI => self.fini = entry.value,
            DT_FINI_ARRAY => self.fini_array = entry.value,
            DT_FINI_ARRAYSZ => self.fini_array_size = entry.value,
            DT_FLAGS => self.flags = entry.value,
            DT_FLAGS_1 => self.flags_1 = entry.value,
            DT_RELA_COUNT => self.rela_count_hint = entry.value,
            DT_REL | DT_RELSZ | DT_RELENT => self.has_rel_table = true,
            DT_NULL | DT_SONAME | DT_RPATH | DT_RUNPATH | DT_BIND_NOW => {}
            _ => {}
        }
        Ok(())
    }

    pub const fn has_rela(&self) -> bool {
        self.rela != 0 && self.rela_size != 0
    }

    pub const fn has_jmprel(&self) -> bool {
        self.jmprel != 0 && self.pltrel_size != 0
    }
}

/// Parse la table `PT_DYNAMIC` déjà mappée dans l'espace utilisateur courant.
///
/// # Safety
/// `dynamic_vaddr` doit pointer vers au moins `max_entries` entrées lisibles,
/// sauf si une entrée `DT_NULL` apparaît avant.
pub unsafe fn parse_dynamic_table(
    dynamic_vaddr: u64,
    max_entries: usize,
) -> Result<DynamicInfo, DynamicError> {
    if dynamic_vaddr == 0 {
        return Ok(DynamicInfo::default());
    }
    if max_entries == 0 {
        return Err(DynamicError::NullTable);
    }

    let entries = core::slice::from_raw_parts(dynamic_vaddr as *const DynamicEntry, max_entries);
    let mut info = DynamicInfo::default();
    let mut idx = 0usize;
    while idx < entries.len() {
        let entry = entries[idx];
        if entry.tag == DT_NULL {
            break;
        }
        info.record(entry)?;
        idx += 1;
    }

    if info.rela_entry_size != 0
        && info.rela_entry_size as usize
            != core::mem::size_of::<crate::elf::relocations::RelaEntry>()
    {
        return Err(DynamicError::InvalidEntrySize);
    }
    if info.syment != 0 && info.syment != 24 {
        return Err(DynamicError::InvalidEntrySize);
    }
    if info.has_rel_table {
        return Err(DynamicError::UnsupportedRelTable);
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rela_and_needed_entries() {
        let dyns = [
            DynamicEntry {
                tag: DT_NEEDED,
                value: 1,
            },
            DynamicEntry {
                tag: DT_RELA,
                value: 0x4000,
            },
            DynamicEntry {
                tag: DT_RELASZ,
                value: 48,
            },
            DynamicEntry {
                tag: DT_RELAENT,
                value: 24,
            },
            DynamicEntry {
                tag: DT_NULL,
                value: 0,
            },
        ];
        let info = unsafe { parse_dynamic_table(dyns.as_ptr() as u64, dyns.len()) }.unwrap();
        assert_eq!(info.needed_count, 1);
        assert_eq!(info.rela, 0x4000);
        assert!(info.has_rela());
    }
}
