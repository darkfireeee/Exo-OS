//! Complete x86-64 `ET_DYN`/`ET_EXEC` ELF64 `RELA` relocation applicator.

use core::ptr;

use crate::dynamic_linker::symbol_table::{cstr_bytes, Elf64Sym, STB_WEAK};
use crate::elf::tls::{exo_tlsdesc_dynamic, exo_tlsdesc_static, TlsDescDynamicArg};

pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_PC32: u32 = 2;
pub const R_X86_64_COPY: u32 = 5;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE: u32 = 8;
pub const R_X86_64_DTPMOD64: u32 = 16;
pub const R_X86_64_DTPOFF64: u32 = 17;
pub const R_X86_64_TPOFF64: u32 = 18;
pub const R_X86_64_TLSDESC: u32 = 36;
pub const R_X86_64_IRELATIVE: u32 = 37;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Elf64Rela {
    pub r_offset: u64,
    pub r_info: u64,
    pub r_addend: i64,
}

impl Elf64Rela {
    #[inline]
    pub fn r_type(&self) -> u32 {
        (self.r_info & 0xffff_ffff) as u32
    }

    #[inline]
    pub fn r_sym(&self) -> u32 {
        (self.r_info >> 32) as u32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedSymbol {
    pub addr: usize,
    pub size: usize,
    pub st_value: u64,
    pub obj_base: usize,
    pub tls_modid: usize,
    pub tls_static_tpoff: isize,
    pub tls_is_static: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct RelocTargetInfo {
    pub obj_idx: usize,
    pub load_bias: usize,
    pub symtab: *const Elf64Sym,
    pub strtab: *const u8,
    pub strsz: usize,
    pub own_tls_modid: usize,
    pub own_tls_static_tpoff: isize,
    pub own_tls_is_static: bool,
}

/// Applies a table of `Elf64Rela` entries (`DT_RELA` or `DT_JMPREL`) using `resolve_sym(name, skip_root, local_fallback)`.
pub unsafe fn apply_rela_table<F>(
    target: &RelocTargetInfo,
    rela_ptr: *const Elf64Rela,
    total_bytes: usize,
    entry_size: usize,
    mut resolve_sym: F,
) -> Result<(), i32>
where
    F: FnMut(&[u8], bool, usize) -> Option<ResolvedSymbol>,
{
    if rela_ptr.is_null() || total_bytes == 0 {
        return Ok(());
    }
    let step = entry_size.max(core::mem::size_of::<Elf64Rela>());
    let count = total_bytes / step;
    for idx in 0..count {
        let rela = unsafe { *((rela_ptr as usize + idx * step) as *const Elf64Rela) };
        let r_type = rela.r_type();
        if r_type == R_X86_64_NONE {
            continue;
        }
        let patch_addr = target.load_bias.wrapping_add(rela.r_offset as usize);
        match r_type {
            R_X86_64_RELATIVE => {
                let val = (target.load_bias as isize).wrapping_add(rela.r_addend as isize) as usize;
                unsafe { ptr::write_unaligned(patch_addr as *mut usize, val) };
            }
            R_X86_64_IRELATIVE => {
                let fn_addr =
                    (target.load_bias as isize).wrapping_add(rela.r_addend as isize) as usize;
                let resolver: extern "C" fn() -> usize = unsafe { core::mem::transmute(fn_addr) };
                let resolved = resolver();
                unsafe { ptr::write_unaligned(patch_addr as *mut usize, resolved) };
            }
            _ => {
                let sym_idx = rela.r_sym();
                let mut is_weak = false;
                let resolved = if sym_idx == 0 {
                    Some(ResolvedSymbol {
                        addr: target.load_bias,
                        size: 0,
                        st_value: 0,
                        obj_base: target.load_bias,
                        tls_modid: target.own_tls_modid,
                        tls_static_tpoff: target.own_tls_static_tpoff,
                        tls_is_static: target.own_tls_is_static,
                    })
                } else {
                    if target.symtab.is_null() || target.strtab.is_null() {
                        return Err(22);
                    }
                    let sym = unsafe { *target.symtab.add(sym_idx as usize) };
                    is_weak = sym.bind() == STB_WEAK;
                    let name = unsafe { cstr_bytes(target.strtab, sym.st_name, target.strsz) };
                    if sym.st_shndx != 0 && (name.is_empty() || !sym.is_exported_def()) {
                        Some(ResolvedSymbol {
                            addr: target.load_bias.wrapping_add(sym.st_value as usize),
                            size: sym.st_size as usize,
                            st_value: sym.st_value,
                            obj_base: target.load_bias,
                            tls_modid: target.own_tls_modid,
                            tls_static_tpoff: target.own_tls_static_tpoff,
                            tls_is_static: target.own_tls_is_static,
                        })
                    } else {
                        let skip_root = r_type == R_X86_64_COPY;
                        let mut found = resolve_sym(name, skip_root, target.obj_idx);
                        if let Some(ref mut f) = found {
                            if f.size == 0 {
                                f.size = sym.st_size as usize;
                            }
                        }
                        found
                    }
                };

                let def = match resolved {
                    Some(d) => d,
                    None if is_weak => ResolvedSymbol {
                        addr: 0,
                        size: 0,
                        st_value: 0,
                        obj_base: 0,
                        tls_modid: 0,
                        tls_static_tpoff: 0,
                        tls_is_static: false,
                    },
                    None => return Err(38), // ENOSYS / missing symbol
                };

                match r_type {
                    R_X86_64_64 => {
                        let val = (def.addr as isize).wrapping_add(rela.r_addend as isize) as u64;
                        unsafe { ptr::write_unaligned(patch_addr as *mut u64, val) };
                    }
                    R_X86_64_PC32 => {
                        let val = (def.addr as i128)
                            .wrapping_add(rela.r_addend as i128)
                            .wrapping_sub(patch_addr as i128);
                        let narrowed = i32::try_from(val).map_err(|_| 34)?; // ERANGE
                        unsafe { ptr::write_unaligned(patch_addr as *mut i32, narrowed) };
                    }
                    R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                        unsafe { ptr::write_unaligned(patch_addr as *mut u64, def.addr as u64) };
                    }
                    R_X86_64_COPY => {
                        if def.addr != 0 && def.size > 0 {
                            unsafe {
                                ptr::copy_nonoverlapping(
                                    def.addr as *const u8,
                                    patch_addr as *mut u8,
                                    def.size,
                                );
                            }
                        }
                    }
                    R_X86_64_DTPMOD64 => {
                        unsafe {
                            ptr::write_unaligned(patch_addr as *mut u64, def.tls_modid as u64)
                        };
                    }
                    R_X86_64_DTPOFF64 => {
                        let val = (def.st_value as i64).wrapping_add(rela.r_addend);
                        unsafe { ptr::write_unaligned(patch_addr as *mut u64, val as u64) };
                    }
                    R_X86_64_TPOFF64 => {
                        if !def.tls_is_static {
                            return Err(22);
                        }
                        let off = (def.st_value as isize)
                            .wrapping_add(rela.r_addend as isize)
                            .wrapping_add(def.tls_static_tpoff);
                        unsafe { ptr::write_unaligned(patch_addr as *mut u64, off as u64) };
                    }
                    R_X86_64_TLSDESC => {
                        let slot = patch_addr as *mut usize;
                        if def.tls_is_static {
                            let off = (def.st_value as isize)
                                .wrapping_add(rela.r_addend as isize)
                                .wrapping_add(def.tls_static_tpoff);
                            unsafe {
                                *slot = exo_tlsdesc_static as *const () as usize;
                                *slot.add(1) = off as usize;
                            }
                        } else {
                            let arg = alloc::boxed::Box::new(TlsDescDynamicArg {
                                mod_id: def.tls_modid,
                                offset: (def.st_value as isize)
                                    .wrapping_add(rela.r_addend as isize)
                                    as usize,
                            });
                            unsafe {
                                *slot = exo_tlsdesc_dynamic as *const () as usize;
                                *slot.add(1) = alloc::boxed::Box::into_raw(arg) as usize;
                            }
                        }
                    }
                    _ => return Err(22),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic_linker::symbol_table::{STB_GLOBAL, STV_DEFAULT};

    extern "C" fn test_ifunc_resolver() -> usize {
        0xCAFE_BABE_0000
    }

    #[test]
    fn applies_relative_irelative_pc32_copy_and_tls_relocs() {
        let mut target_mem = [0u64; 16];
        let base = target_mem.as_mut_ptr() as usize;
        let src_bytes: [u8; 4] = [0x11, 0x22, 0x33, 0x44];
        let strtab = b"\0ext_data\0tls_var\0";
        let symtab = [
            Elf64Sym {
                st_name: 0,
                st_info: 0,
                st_other: 0,
                st_shndx: 0,
                st_value: 0,
                st_size: 0,
            },
            Elf64Sym {
                st_name: 1,
                st_info: STB_GLOBAL << 4,
                st_other: STV_DEFAULT,
                st_shndx: 0,
                st_value: 0,
                st_size: 4,
            },
            Elf64Sym {
                st_name: 10,
                st_info: (STB_GLOBAL << 4) | 6,
                st_other: STV_DEFAULT,
                st_shndx: 1,
                st_value: 8,
                st_size: 8,
            },
        ];

        let ifunc_offset = (test_ifunc_resolver as *const () as usize).wrapping_sub(base);
        let relas = [
            Elf64Rela { r_offset: 0, r_info: R_X86_64_RELATIVE as u64, r_addend: 0x100 },
            Elf64Rela { r_offset: 8, r_info: R_X86_64_IRELATIVE as u64, r_addend: ifunc_offset as i64 },
            Elf64Rela { r_offset: 16, r_info: (1u64 << 32) | (R_X86_64_COPY as u64), r_addend: 0 },
            Elf64Rela { r_offset: 24, r_info: (2u64 << 32) | (R_X86_64_DTPMOD64 as u64), r_addend: 0 },
            Elf64Rela { r_offset: 32, r_info: (2u64 << 32) | (R_X86_64_DTPOFF64 as u64), r_addend: 4 },
            Elf64Rela { r_offset: 40, r_info: (2u64 << 32) | (R_X86_64_TPOFF64 as u64), r_addend: 0 },
            Elf64Rela { r_offset: 48, r_info: (1u64 << 32) | (R_X86_64_PC32 as u64), r_addend: 0 },
        ];
        let info = RelocTargetInfo {
            obj_idx: 0,
            load_bias: base,
            symtab: symtab.as_ptr(),
            strtab: strtab.as_ptr(),
            strsz: strtab.len(),
            own_tls_modid: 1,
            own_tls_static_tpoff: -64,
            own_tls_is_static: true,
        };
        unsafe {
            apply_rela_table(
                &info,
                relas.as_ptr(),
                core::mem::size_of_val(&relas),
                core::mem::size_of::<Elf64Rela>(),
                |name, skip_root, _| match name {
                    b"ext_data" => {
                        assert!(skip_root || name == b"ext_data");
                        Some(ResolvedSymbol {
                            addr: if skip_root {
                                src_bytes.as_ptr() as usize
                            } else {
                                base + 52
                            },
                            size: 4,
                            st_value: 0,
                            obj_base: 0,
                            tls_modid: 0,
                            tls_static_tpoff: 0,
                            tls_is_static: false,
                        })
                    }
                    b"tls_var" => Some(ResolvedSymbol {
                        addr: 8,
                        size: 8,
                        st_value: 8,
                        obj_base: 0,
                        tls_modid: 2,
                        tls_static_tpoff: -64,
                        tls_is_static: true,
                    }),
                    _ => None,
                },
            )
            .unwrap();
        }
        assert_eq!(target_mem[0], (base + 0x100) as u64);
        assert_eq!(target_mem[1], 0xCAFE_BABE_0000);
        assert_eq!(target_mem[2] as u32, u32::from_le_bytes(src_bytes));
        assert_eq!(target_mem[3], 2);
        assert_eq!(target_mem[4], 12);
        assert_eq!(target_mem[5] as i64, -56);
        assert_eq!((target_mem[6] as u32) as i32, 4); // (base + 52) - (base + 48) = +4
    }
}
