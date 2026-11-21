//! ELF64 dynamic symbol table lookup supporting SysV `DT_HASH` and GNU `DT_GNU_HASH`.

pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;
pub const STB_GNU_UNIQUE: u8 = 10;

pub const STT_TLS: u8 = 6;
pub const STT_GNU_IFUNC: u8 = 10;

pub const STV_DEFAULT: u8 = 0;
pub const STV_INTERNAL: u8 = 1;
pub const STV_HIDDEN: u8 = 2;
pub const STV_PROTECTED: u8 = 3;

pub const SHN_UNDEF: u16 = 0;
pub const SHN_ABS: u16 = 0xfff1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Elf64Sym {
    pub st_name: u32,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size: u64,
}

const _: () = assert!(core::mem::size_of::<Elf64Sym>() == 24);

impl Elf64Sym {
    #[inline]
    pub fn bind(&self) -> u8 {
        self.st_info >> 4
    }

    #[inline]
    pub fn sym_type(&self) -> u8 {
        self.st_info & 0xf
    }

    #[inline]
    pub fn visibility(&self) -> u8 {
        self.st_other & 0x3
    }

    #[inline]
    pub fn is_exported_def(&self) -> bool {
        if self.st_shndx == SHN_UNDEF {
            return false;
        }
        let vis = self.visibility();
        if vis == STV_HIDDEN || vis == STV_INTERNAL {
            return false;
        }
        matches!(self.bind(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
    }
}

/// Standard SysV ELF hash.
#[inline]
pub fn elf_hash(name: &[u8]) -> u32 {
    let mut h = 0u32;
    for &b in name {
        h = (h << 4).wrapping_add(b as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// GNU ELF symbol hash (`djb2`).
#[inline]
pub fn gnu_hash(name: &[u8]) -> u32 {
    let mut h = 5381u32;
    for &b in name {
        h = h.wrapping_mul(33).wrapping_add(b as u32);
    }
    h
}

/// Reads a NUL-terminated C string at `strtab + offset` safely up to `max_scan`.
#[inline]
pub unsafe fn cstr_bytes<'a>(strtab: *const u8, offset: u32, strsz: usize) -> &'a [u8] {
    if strtab.is_null() {
        return &[];
    }
    let off = offset as usize;
    if strsz > 0 && off >= strsz {
        return &[];
    }
    let limit = if strsz > 0 { strsz - off } else { 4096 };
    let ptr = unsafe { strtab.add(off) };
    let mut len = 0usize;
    while len < limit {
        if unsafe { *ptr.add(len) } == 0 {
            break;
        }
        len += 1;
    }
    unsafe { core::slice::from_raw_parts(ptr, len) }
}

/// Looks up `name` in SysV `DT_HASH`.
pub unsafe fn lookup_sysv_hash(
    name: &[u8],
    hash_ptr: *const u32,
    symtab: *const Elf64Sym,
    strtab: *const u8,
    strsz: usize,
) -> Option<(u32, Elf64Sym)> {
    if hash_ptr.is_null() || symtab.is_null() || strtab.is_null() {
        return None;
    }
    let nbuckets = unsafe { *hash_ptr } as usize;
    let nchain = unsafe { *hash_ptr.add(1) } as usize;
    if nbuckets == 0 || nchain == 0 {
        return None;
    }
    let buckets = unsafe { hash_ptr.add(2) };
    let chains = unsafe { buckets.add(nbuckets) };
    let h = elf_hash(name);
    let mut sym_idx = unsafe { *buckets.add((h as usize) % nbuckets) } as usize;
    let mut steps = 0usize;
    while sym_idx != 0 && sym_idx < nchain && steps < nchain {
        let sym = unsafe { *symtab.add(sym_idx) };
        if sym.is_exported_def() {
            let sym_name = unsafe { cstr_bytes(strtab, sym.st_name, strsz) };
            if sym_name == name {
                return Some((sym_idx as u32, sym));
            }
        }
        sym_idx = unsafe { *chains.add(sym_idx) } as usize;
        steps += 1;
    }
    None
}

/// Looks up `name` in GNU `DT_GNU_HASH` (ELF64 64-bit bloom filter).
pub unsafe fn lookup_gnu_hash(
    name: &[u8],
    gnu_hash_ptr: *const u32,
    symtab: *const Elf64Sym,
    strtab: *const u8,
    strsz: usize,
) -> Option<(u32, Elf64Sym)> {
    if gnu_hash_ptr.is_null() || symtab.is_null() || strtab.is_null() {
        return None;
    }
    let nbuckets = unsafe { *gnu_hash_ptr } as usize;
    let symoffset = unsafe { *gnu_hash_ptr.add(1) } as usize;
    let bloom_size = unsafe { *gnu_hash_ptr.add(2) } as usize;
    let bloom_shift = unsafe { *gnu_hash_ptr.add(3) };
    if nbuckets == 0 || bloom_size == 0 {
        return None;
    }
    let bloom = unsafe { gnu_hash_ptr.add(4) as *const u64 };
    let buckets = unsafe { bloom.add(bloom_size) as *const u32 };
    let chain = unsafe { buckets.add(nbuckets) };

    let h = gnu_hash(name);
    let word = unsafe { *bloom.add(((h as usize) / 64) % bloom_size) };
    let mask = (1u64 << (h % 64)) | (1u64 << ((h >> bloom_shift) % 64));
    if (word & mask) != mask {
        return None;
    }

    let mut sym_idx = unsafe { *buckets.add((h as usize) % nbuckets) } as usize;
    if sym_idx < symoffset {
        return None;
    }
    let mut guard = 0usize;
    while guard < 1_048_576 {
        let h2 = unsafe { *chain.add(sym_idx - symoffset) };
        if (h2 | 1) == (h | 1) {
            let sym = unsafe { *symtab.add(sym_idx) };
            if sym.is_exported_def() {
                let sym_name = unsafe { cstr_bytes(strtab, sym.st_name, strsz) };
                if sym_name == name {
                    return Some((sym_idx as u32, sym));
                }
            }
        }
        if (h2 & 1) != 0 {
            break;
        }
        sym_idx += 1;
        guard += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn sysv_and_gnu_hash_functions_match_standard_vectors() {
        assert_eq!(elf_hash(b""), 0);
        assert_eq!(elf_hash(b"printf"), 0x0779_05a6);
        assert_eq!(gnu_hash(b""), 5381);
        assert_eq!(gnu_hash(b"printf"), 0x156b_2bb8);
    }

    #[test]
    fn sysv_hash_lookup_finds_defined_and_skips_undef() {
        let strtab = b"\0missing\0my_func\0";
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
                st_name: 1, // "missing"
                st_info: (STB_GLOBAL << 4) | 2,
                st_other: STV_DEFAULT,
                st_shndx: SHN_UNDEF,
                st_value: 0,
                st_size: 0,
            },
            Elf64Sym {
                st_name: 9, // "my_func"
                st_info: (STB_WEAK << 4) | 2,
                st_other: STV_DEFAULT,
                st_shndx: 1,
                st_value: 0x1234,
                st_size: 16,
            },
        ];
        // 1 bucket, 3 chains
        let hash = [1u32, 3, 1, 0, 0, 2]; // wait chain[1]=2 -> let's build [nbucket=1, nchain=3, bucket0=1, chain0=0, chain1=2, chain2=0]
        let mut tbl = vec![1u32, 3, 1, 0, 2, 0];
        let found = unsafe {
            lookup_sysv_hash(b"my_func", tbl.as_ptr(), symtab.as_ptr(), strtab.as_ptr(), strtab.len())
        };
        assert_eq!(found.map(|(idx, s)| (idx, s.st_value)), Some((2, 0x1234)));
        let undef = unsafe {
            lookup_sysv_hash(b"missing", tbl.as_ptr(), symtab.as_ptr(), strtab.as_ptr(), strtab.len())
        };
        assert!(undef.is_none());
        let _ = hash;
        let _ = &mut tbl;
    }

    #[test]
    fn gnu_hash_lookup_finds_bloom_and_chain_symbol() {
        let strtab = b"\0target_sym\0";
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
                st_info: (STB_GLOBAL << 4) | 2,
                st_other: STV_DEFAULT,
                st_shndx: 2,
                st_value: 0x5678,
                st_size: 8,
            },
        ];
        let h = gnu_hash(b"target_sym");
        let bloom_mask = (1u64 << (h % 64)) | (1u64 << ((h >> 5) % 64));
        // u32 header [nbuckets=1, symoffset=1, bloom_size=1, bloom_shift=5] + bloom (2 u32s) + buckets[1] + chain[1]
        let mut table = vec![
            1u32,
            1,
            1,
            5,
            (bloom_mask & 0xffff_ffff) as u32,
            (bloom_mask >> 32) as u32,
            1,       // bucket[0] -> sym 1
            h | 1,   // chain[0] -> ends chain
        ];
        let found = unsafe {
            lookup_gnu_hash(
                b"target_sym",
                table.as_ptr(),
                symtab.as_ptr(),
                strtab.as_ptr(),
                strtab.len(),
            )
        };
        assert_eq!(found.map(|(idx, s)| (idx, s.st_value)), Some((1, 0x5678)));
        let nope = unsafe {
            lookup_gnu_hash(
                b"other_sym",
                table.as_ptr(),
                symtab.as_ptr(),
                strtab.as_ptr(),
                strtab.len(),
            )
        };
        assert!(nope.is_none());
        let _ = &mut table;
    }
}
