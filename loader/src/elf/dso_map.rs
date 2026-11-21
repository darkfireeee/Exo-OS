//! File-backed ELF64 DSO mapper (`mmap(PROT_NONE)` span + `MAP_FIXED` per-`PT_LOAD` + `PT_GNU_RELRO` + strict W^X).

use alloc::vec;
use alloc::vec::Vec;

use crate::elf::parser::{
    read_u16, read_u32, read_u64, ELF_MAGIC, ET_DYN, ET_EXEC, PT_DYNAMIC, PT_GNU_RELRO, PT_LOAD,
    PT_PHDR, PT_TLS,
};
use crate::syscall::{
    sys_mmap, sys_mprotect, sys_munmap, sys_pread_exact, MAP_ANONYMOUS, MAP_FIXED, MAP_PRIVATE,
    PROT_EXEC, PROT_NONE, PROT_READ, PROT_WRITE,
};

const PAGE_SIZE: usize = 4096;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

#[derive(Clone, Copy, Debug)]
pub struct ParsedPhdr {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

#[derive(Clone, Debug)]
pub struct MappedElfLayout {
    pub span_base: usize,
    pub span_len: usize,
    pub load_bias: usize,
    pub entry_vaddr: usize,
    pub phdr_vaddr: usize,
    pub phnum: u16,
    pub dynamic_vaddr: usize,
    pub relro_start: usize,
    pub relro_len: usize,
    pub tls: Option<ParsedPhdr>,
    pub phdrs_storage: Vec<ParsedPhdr>,
}

#[inline]
pub fn pflags_to_prot(flags: u32) -> Result<u32, i32> {
    if (flags & (PF_W | PF_X)) == (PF_W | PF_X) {
        return Err(13); // EACCES: W^X
    }
    let mut prot = PROT_NONE;
    if (flags & PF_R) != 0 {
        prot |= PROT_READ;
    }
    if (flags & PF_W) != 0 {
        prot |= PROT_WRITE;
    }
    if (flags & PF_X) != 0 {
        prot |= PROT_EXEC;
    }
    Ok(prot)
}

#[inline]
fn page_down(v: usize) -> usize {
    v & !(PAGE_SIZE - 1)
}

#[inline]
fn page_up(v: usize) -> Option<usize> {
    v.checked_add(PAGE_SIZE - 1).map(|x| x & !(PAGE_SIZE - 1))
}

/// Parses ELF64 header and program headers from `fd` via `pread64` and validates `ET_DYN`/`ET_EXEC`.
pub fn read_phdrs_from_fd(fd: i32) -> Result<(u16, u64, usize, Vec<ParsedPhdr>), i32> {
    let mut ehdr = [0u8; 64];
    unsafe { sys_pread_exact(fd, &mut ehdr, 0)? };
    if &ehdr[0..4] != ELF_MAGIC || ehdr[4] != 2 || ehdr[5] != 1 {
        return Err(8); // ENOEXEC
    }
    let e_type = read_u16(&ehdr, 16);
    if e_type != ET_DYN && e_type != ET_EXEC {
        return Err(8);
    }
    if read_u16(&ehdr, 18) != 62 {
        return Err(8);
    }
    let e_entry = read_u64(&ehdr, 24);
    let e_phoff = read_u64(&ehdr, 32);
    let e_phentsize = read_u16(&ehdr, 54) as usize;
    let e_phnum = read_u16(&ehdr, 56) as usize;
    if e_phentsize < 56 || e_phnum == 0 || e_phnum > 128 {
        return Err(8);
    }

    let table_len = e_phentsize.checked_mul(e_phnum).ok_or(22)?;
    let mut raw = vec![0u8; table_len];
    unsafe { sys_pread_exact(fd, &mut raw, e_phoff)? };

    let mut phdrs = Vec::with_capacity(e_phnum);
    for idx in 0..e_phnum {
        let off = idx * e_phentsize;
        phdrs.push(ParsedPhdr {
            p_type: read_u32(&raw, off),
            flags: read_u32(&raw, off + 4),
            offset: read_u64(&raw, off + 8),
            vaddr: read_u64(&raw, off + 16),
            filesz: read_u64(&raw, off + 32),
            memsz: read_u64(&raw, off + 40),
            align: read_u64(&raw, off + 48),
        });
    }
    Ok((e_type, e_entry, e_phoff as usize, phdrs))
}

/// Computes `(min_vaddr_aligned, max_vaddr_aligned)` across all `PT_LOAD` segments.
pub fn compute_load_span(phdrs: &[ParsedPhdr]) -> Result<(usize, usize), i32> {
    let mut min_v = usize::MAX;
    let mut max_v = 0usize;
    for ph in phdrs {
        if ph.p_type != PT_LOAD || ph.memsz == 0 {
            continue;
        }
        let _ = pflags_to_prot(ph.flags)?;
        let start = page_down(ph.vaddr as usize);
        let end = page_up((ph.vaddr as usize).checked_add(ph.memsz as usize).ok_or(22)?).ok_or(22)?;
        if start < min_v {
            min_v = start;
        }
        if end > max_v {
            max_v = end;
        }
    }
    if min_v == usize::MAX || max_v <= min_v {
        return Err(8);
    }
    Ok((min_v, max_v))
}

/// Computes safe page-aligned `(relro_start, relro_len)` intersecting writable `PT_LOAD` segments.
pub fn compute_relro_bounds(phdrs: &[ParsedPhdr], load_bias: usize) -> (usize, usize) {
    let Some(relro) = phdrs.iter().find(|p| p.p_type == PT_GNU_RELRO && p.memsz > 0) else {
        return (0, 0);
    };
    let raw_start = load_bias.wrapping_add(relro.vaddr as usize);
    let raw_end = raw_start.wrapping_add(relro.memsz as usize);
    let starts_at_seg_page = phdrs.iter().any(|p| {
        p.p_type == PT_LOAD
            && (relro.vaddr & !0xFFF) == (p.vaddr & !0xFFF)
            && relro.vaddr >= p.vaddr
    });
    let start_aligned = if starts_at_seg_page {
        page_down(raw_start)
    } else {
        page_up(raw_start).unwrap_or(raw_start)
    };
    let end_aligned = page_down(raw_end);
    if end_aligned > start_aligned {
        (start_aligned, end_aligned - start_aligned)
    } else {
        (0, 0)
    }
}

/// Maps an ELF object from `fd` into memory with `mmap(PROT_NONE)` span + `MAP_FIXED` `PT_LOAD` segments.
pub fn map_elf_from_fd(fd: i32) -> Result<MappedElfLayout, i32> {
    let (e_type, e_entry, e_phoff, phdrs) = read_phdrs_from_fd(fd)?;
    let (min_vaddr, max_vaddr) = compute_load_span(&phdrs)?;
    let span_len = max_vaddr - min_vaddr;

    let hint_addr = if e_type == ET_EXEC { min_vaddr } else { 0 };
    let hint_flags = if e_type == ET_EXEC {
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED
    } else {
        MAP_PRIVATE | MAP_ANONYMOUS
    };
    let span_base = unsafe { sys_mmap(hint_addr, span_len, PROT_NONE, hint_flags, -1, 0)? };
    let load_bias = span_base.wrapping_sub(min_vaddr);

    let result = map_segments_into_span(fd, span_base, span_len, load_bias, e_entry, e_phoff, &phdrs);
    if result.is_err() {
        unsafe {
            let _ = sys_munmap(span_base, span_len);
        }
    }
    result
}

fn map_segments_into_span(
    fd: i32,
    span_base: usize,
    span_len: usize,
    load_bias: usize,
    e_entry: u64,
    e_phoff: usize,
    phdrs: &[ParsedPhdr],
) -> Result<MappedElfLayout, i32> {
    let mut pt_phdr_vaddr = 0usize;
    let mut first_load_phdr = 0usize;
    let mut dynamic_vaddr = 0usize;
    let mut tls_phdr: Option<ParsedPhdr> = None;

    for ph in phdrs {
        match ph.p_type {
            PT_PHDR => pt_phdr_vaddr = load_bias.wrapping_add(ph.vaddr as usize),
            PT_DYNAMIC => dynamic_vaddr = load_bias.wrapping_add(ph.vaddr as usize),
            PT_TLS => {
                if ph.memsz > 0 {
                    tls_phdr = Some(*ph);
                }
            }
            PT_LOAD if ph.memsz > 0 => {
                if ph.filesz > ph.memsz {
                    return Err(8);
                }
                if (ph.vaddr & 0xFFF) != (ph.offset & 0xFFF) {
                    return Err(8);
                }
                let prot = pflags_to_prot(ph.flags)?;
                let seg_vaddr = load_bias.wrapping_add(ph.vaddr as usize);
                let seg_start = page_down(seg_vaddr);
                let file_off_aligned = ph.offset & !0xFFF;
                let file_end = seg_vaddr + ph.filesz as usize;
                let file_map_end = page_up(file_end).ok_or(22)?;
                let mem_map_end = page_up(seg_vaddr + ph.memsz as usize).ok_or(22)?;

                if first_load_phdr == 0
                    && ph.offset as usize <= e_phoff
                    && e_phoff < (ph.offset as usize).saturating_add(ph.filesz as usize)
                {
                    first_load_phdr = seg_vaddr + (e_phoff - ph.offset as usize);
                }

                if ph.filesz > 0 && file_map_end > seg_start {
                    let _ = unsafe {
                        sys_mmap(
                            seg_start,
                            file_map_end - seg_start,
                            prot,
                            MAP_PRIVATE | MAP_FIXED,
                            fd,
                            file_off_aligned,
                        )?
                    };
                    if (ph.flags & PF_W) != 0 && file_map_end > file_end {
                        unsafe {
                            core::ptr::write_bytes(file_end as *mut u8, 0, file_map_end - file_end);
                        }
                    }
                }

                let anon_start = if ph.filesz == 0 {
                    seg_start
                } else {
                    file_map_end
                };
                if mem_map_end > anon_start {
                    let _ = unsafe {
                        sys_mmap(
                            anon_start,
                            mem_map_end - anon_start,
                            prot,
                            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                            -1,
                            0,
                        )?
                    };
                }
            }
            _ => {}
        }
    }

    let phdr_vaddr = if pt_phdr_vaddr != 0 {
        pt_phdr_vaddr
    } else if first_load_phdr != 0 {
        first_load_phdr
    } else {
        span_base.wrapping_add(e_phoff)
    };
    let (relro_start, relro_len) = compute_relro_bounds(phdrs, load_bias);

    Ok(MappedElfLayout {
        span_base,
        span_len,
        load_bias,
        entry_vaddr: if e_entry == 0 {
            0
        } else {
            load_bias.wrapping_add(e_entry as usize)
        },
        phdr_vaddr,
        phnum: phdrs.len() as u16,
        dynamic_vaddr,
        relro_start,
        relro_len,
        tls: tls_phdr,
        phdrs_storage: phdrs.to_vec(),
    })
}

/// Enforces `PT_GNU_RELRO` read-only protection after GOT/RELRO relocations complete.
pub fn protect_relro(relro_start: usize, relro_len: usize) -> Result<(), i32> {
    if relro_start != 0 && relro_len != 0 {
        unsafe { sys_mprotect(relro_start, relro_len, PROT_READ)? };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_rwx_and_computes_relro_safely() {
        assert_eq!(pflags_to_prot(PF_R | PF_W | PF_X), Err(13));
        assert_eq!(pflags_to_prot(PF_R | PF_X), Ok(PROT_READ | PROT_EXEC));

        let phdrs = [
            ParsedPhdr {
                p_type: PT_LOAD,
                flags: PF_R | PF_W,
                offset: 0x2000,
                vaddr: 0x2000,
                filesz: 0x1500,
                memsz: 0x2000,
                align: 0x1000,
            },
            ParsedPhdr {
                p_type: PT_GNU_RELRO,
                flags: PF_R,
                offset: 0x2000,
                vaddr: 0x2000,
                filesz: 0x1000,
                memsz: 0x1000,
                align: 1,
            },
        ];
        assert_eq!(compute_load_span(&phdrs), Ok((0x2000, 0x4000)));
        assert_eq!(compute_relro_bounds(&phdrs, 0x10000), (0x12000, 0x1000));
    }
}
