//! Full ELF64 `PT_DYNAMIC` parser producing relocated pointers for the dynamic linker.

use alloc::vec::Vec;

use crate::dynamic_linker::symbol_table::Elf64Sym;
use crate::elf::reloc_apply::Elf64Rela;

pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_HASH: i64 = 4;
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
pub const DT_SYMBOLIC: i64 = 16;
pub const DT_PLTREL: i64 = 20;
pub const DT_DEBUG: i64 = 21;
pub const DT_TEXTREL: i64 = 22;
pub const DT_JMPREL: i64 = 23;
pub const DT_BIND_NOW: i64 = 24;
pub const DT_INIT_ARRAY: i64 = 25;
pub const DT_FINI_ARRAY: i64 = 26;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_FINI_ARRAYSZ: i64 = 28;
pub const DT_RUNPATH: i64 = 29;
pub const DT_FLAGS: i64 = 30;
pub const DT_PREINIT_ARRAY: i64 = 32;
pub const DT_PREINIT_ARRAYSZ: i64 = 33;
pub const DT_GNU_HASH: i64 = 0x6fff_fef5;
pub const DT_RELACOUNT: i64 = 0x6fff_fff9;
pub const DT_FLAGS_1: i64 = 0x6fff_fffb;

pub const DF_ORIGIN: u64 = 0x1;
pub const DF_SYMBOLIC: u64 = 0x2;
pub const DF_TEXTREL: u64 = 0x4;
pub const DF_BIND_NOW: u64 = 0x8;
pub const DF_STATIC_TLS: u64 = 0x10;
pub const DF_1_NOW: u64 = 0x1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Elf64Dyn {
    pub d_tag: i64,
    pub d_val: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DynamicInfo {
    pub needed_offs: Vec<u32>,
    pub soname_off: Option<u32>,
    pub rpath_off: Option<u32>,
    pub runpath_off: Option<u32>,
    pub strtab: usize,
    pub strsz: usize,
    pub symtab: usize,
    pub syment: usize,
    pub hash: usize,
    pub gnu_hash: usize,
    pub rela: usize,
    pub relasz: usize,
    pub relaent: usize,
    pub jmprel: usize,
    pub pltrelsz: usize,
    pub pltrel_type: i64,
    pub init: usize,
    pub fini: usize,
    pub init_array: usize,
    pub init_arraysz: usize,
    pub fini_array: usize,
    pub fini_arraysz: usize,
    pub preinit_array: usize,
    pub preinit_arraysz: usize,
    pub flags: u64,
    pub flags_1: u64,
    pub has_symbolic: bool,
    pub has_textrel: bool,
    pub bind_now: bool,
    pub debug_slot: usize,
}

impl DynamicInfo {
    /// Parses an in-memory `PT_DYNAMIC` table at `dyn_vaddr` applying `load_bias` to address tags.
    ///
    /// # Safety
    /// `dyn_vaddr` must point to a readable `DT_NULL`-terminated `Elf64Dyn` array.
    pub unsafe fn from_dyn_ptr(dyn_vaddr: usize, load_bias: usize) -> Result<Self, i32> {
        if dyn_vaddr == 0 {
            return Ok(Self::default());
        }
        let mut info = Self::default();
        let dyn_ptr = dyn_vaddr as *const Elf64Dyn;
        let mut raw_strtab = 0u64;
        let mut raw_symtab = 0u64;
        let mut raw_hash = 0u64;
        let mut raw_gnu_hash = 0u64;
        let mut raw_rela = 0u64;
        let mut raw_jmprel = 0u64;
        let mut raw_init = 0u64;
        let mut raw_fini = 0u64;
        let mut raw_init_array = 0u64;
        let mut raw_fini_array = 0u64;
        let mut raw_preinit_array = 0u64;

        let mut i = 0usize;
        while i < 4096 {
            let entry = unsafe { *dyn_ptr.add(i) };
            match entry.d_tag {
                DT_NULL => break,
                DT_NEEDED => info.needed_offs.push(entry.d_val as u32),
                DT_SONAME => info.soname_off = Some(entry.d_val as u32),
                DT_RPATH => info.rpath_off = Some(entry.d_val as u32),
                DT_RUNPATH => info.runpath_off = Some(entry.d_val as u32),
                DT_STRTAB => raw_strtab = entry.d_val,
                DT_STRSZ => info.strsz = entry.d_val as usize,
                DT_SYMTAB => raw_symtab = entry.d_val,
                DT_SYMENT => info.syment = entry.d_val as usize,
                DT_HASH => raw_hash = entry.d_val,
                DT_GNU_HASH => raw_gnu_hash = entry.d_val,
                DT_RELA => raw_rela = entry.d_val,
                DT_RELASZ => info.relasz = entry.d_val as usize,
                DT_RELAENT => info.relaent = entry.d_val as usize,
                DT_JMPREL => raw_jmprel = entry.d_val,
                DT_PLTRELSZ => info.pltrelsz = entry.d_val as usize,
                DT_PLTREL => info.pltrel_type = entry.d_val as i64,
                DT_INIT => raw_init = entry.d_val,
                DT_FINI => raw_fini = entry.d_val,
                DT_INIT_ARRAY => raw_init_array = entry.d_val,
                DT_INIT_ARRAYSZ => info.init_arraysz = entry.d_val as usize,
                DT_FINI_ARRAY => raw_fini_array = entry.d_val,
                DT_FINI_ARRAYSZ => info.fini_arraysz = entry.d_val as usize,
                DT_PREINIT_ARRAY => raw_preinit_array = entry.d_val,
                DT_PREINIT_ARRAYSZ => info.preinit_arraysz = entry.d_val as usize,
                DT_SYMBOLIC => info.has_symbolic = true,
                DT_TEXTREL => info.has_textrel = true,
                DT_BIND_NOW => info.bind_now = true,
                DT_FLAGS => {
                    info.flags = entry.d_val;
                    if (entry.d_val & DF_SYMBOLIC) != 0 {
                        info.has_symbolic = true;
                    }
                    if (entry.d_val & DF_TEXTREL) != 0 {
                        info.has_textrel = true;
                    }
                    if (entry.d_val & DF_BIND_NOW) != 0 {
                        info.bind_now = true;
                    }
                }
                DT_FLAGS_1 => {
                    info.flags_1 = entry.d_val;
                    if (entry.d_val & DF_1_NOW) != 0 {
                        info.bind_now = true;
                    }
                }
                DT_DEBUG => {
                    info.debug_slot = unsafe { &(*dyn_ptr.add(i)).d_val as *const u64 as usize };
                }
                _ => {}
            }
            i += 1;
        }

        if info.has_textrel {
            return Err(13); // EACCES: W^X forbids DT_TEXTREL
        }

        let reloc = |val: u64| -> usize {
            if val == 0 {
                0
            } else {
                load_bias.wrapping_add(val as usize)
            }
        };
        info.strtab = reloc(raw_strtab);
        info.symtab = reloc(raw_symtab);
        info.hash = reloc(raw_hash);
        info.gnu_hash = reloc(raw_gnu_hash);
        info.rela = reloc(raw_rela);
        info.jmprel = reloc(raw_jmprel);
        info.init = reloc(raw_init);
        info.fini = reloc(raw_fini);
        info.init_array = reloc(raw_init_array);
        info.fini_array = reloc(raw_fini_array);
        info.preinit_array = reloc(raw_preinit_array);
        if info.syment == 0 {
            info.syment = core::mem::size_of::<Elf64Sym>();
        }
        if info.relaent == 0 {
            info.relaent = core::mem::size_of::<Elf64Rela>();
        }
        Ok(info)
    }

    pub unsafe fn str_at(&self, off: u32) -> Option<&str> {
        if self.strtab == 0 {
            return None;
        }
        let bytes = unsafe {
            crate::dynamic_linker::symbol_table::cstr_bytes(
                self.strtab as *const u8,
                off,
                self.strsz,
            )
        };
        core::str::from_utf8(bytes).ok()
    }

    pub unsafe fn needed_names(&self) -> Vec<&str> {
        let mut out = Vec::with_capacity(self.needed_offs.len());
        for &off in &self.needed_offs {
            if let Some(s) = unsafe { self.str_at(off) } {
                out.push(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_info_rejects_textrel_and_parses_tags() {
        let strtab = b"\0libc.so\0libself.so\0/opt\0";
        let dyn_ok = [
            Elf64Dyn { d_tag: DT_STRTAB, d_val: strtab.as_ptr() as u64 },
            Elf64Dyn { d_tag: DT_STRSZ, d_val: strtab.len() as u64 },
            Elf64Dyn { d_tag: DT_NEEDED, d_val: 1 },
            Elf64Dyn { d_tag: DT_SONAME, d_val: 9 },
            Elf64Dyn { d_tag: DT_RUNPATH, d_val: 20 },
            Elf64Dyn { d_tag: DT_FLAGS, d_val: DF_SYMBOLIC | DF_BIND_NOW },
            Elf64Dyn { d_tag: DT_NULL, d_val: 0 },
        ];
        let info = unsafe { DynamicInfo::from_dyn_ptr(dyn_ok.as_ptr() as usize, 0) }.unwrap();
        assert_eq!(unsafe { info.needed_names() }, alloc::vec!["libc.so"]);
        assert_eq!(unsafe { info.str_at(info.soname_off.unwrap()) }, Some("libself.so"));
        assert_eq!(unsafe { info.str_at(info.runpath_off.unwrap()) }, Some("/opt"));
        assert!(info.has_symbolic && info.bind_now);

        let dyn_bad = [
            Elf64Dyn { d_tag: DT_FLAGS, d_val: DF_TEXTREL },
            Elf64Dyn { d_tag: DT_NULL, d_val: 0 },
        ];
        assert_eq!(
            unsafe { DynamicInfo::from_dyn_ptr(dyn_bad.as_ptr() as usize, 0) },
            Err(13)
        );
    }
}
