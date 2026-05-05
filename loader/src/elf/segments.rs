use super::parser::{program_header, ElfError, ElfHeader, PT_LOAD};

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SegmentFlags {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl SegmentFlags {
    pub const fn from_elf(flags: u32) -> Self {
        Self {
            read: flags & PF_R != 0,
            write: flags & PF_W != 0,
            execute: flags & PF_X != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadSegment {
    pub file_offset: u64,
    pub virt_addr: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
    pub flags: SegmentFlags,
}

pub struct SegmentTable<const N: usize> {
    segments: [Option<LoadSegment>; N],
    len: usize,
}

impl<const N: usize> Default for SegmentTable<N> {
    fn default() -> Self {
        Self {
            segments: [None; N],
            len: 0,
        }
    }
}

impl<const N: usize> SegmentTable<N> {
    pub fn parse(image: &[u8], header: ElfHeader) -> Result<Self, ElfError> {
        let mut table = Self::default();
        for idx in 0..header.phnum {
            let ph = program_header(image, header, idx)?;
            if ph.p_type != PT_LOAD {
                continue;
            }
            if ph.filesz > ph.memsz {
                return Err(ElfError::BadProgramHeaderTable);
            }
            let file_end = (ph.offset as usize)
                .checked_add(ph.filesz as usize)
                .ok_or(ElfError::ArithmeticOverflow)?;
            if file_end > image.len() {
                return Err(ElfError::BadProgramHeaderTable);
            }
            if table.len == N {
                return Err(ElfError::BadProgramHeaderTable);
            }
            table.segments[table.len] = Some(LoadSegment {
                file_offset: ph.offset,
                virt_addr: ph.vaddr,
                file_size: ph.filesz,
                mem_size: ph.memsz,
                align: ph.align,
                flags: SegmentFlags::from_elf(ph.flags),
            });
            table.len += 1;
        }
        Ok(table)
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub fn iter(&self) -> impl Iterator<Item = LoadSegment> + '_ {
        self.segments[..self.len].iter().filter_map(|entry| *entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::parser::{parse_header, ELF_MAGIC, PT_LOAD};

    #[test]
    fn extracts_load_segment() {
        let mut img = [0u8; 288];
        img[0..4].copy_from_slice(ELF_MAGIC);
        img[4] = 2;
        img[5] = 1;
        img[6] = 1;
        img[16..18].copy_from_slice(&2u16.to_le_bytes());
        img[18..20].copy_from_slice(&62u16.to_le_bytes());
        img[20..24].copy_from_slice(&1u32.to_le_bytes());
        img[32..40].copy_from_slice(&64u64.to_le_bytes());
        img[54..56].copy_from_slice(&56u16.to_le_bytes());
        img[56..58].copy_from_slice(&1u16.to_le_bytes());
        img[64..68].copy_from_slice(&PT_LOAD.to_le_bytes());
        img[68..72].copy_from_slice(&5u32.to_le_bytes());
        img[72..80].copy_from_slice(&256u64.to_le_bytes());
        img[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        img[96..104].copy_from_slice(&16u64.to_le_bytes());
        img[104..112].copy_from_slice(&32u64.to_le_bytes());
        let hdr = parse_header(&img).unwrap();
        let table = SegmentTable::<4>::parse(&img, hdr).unwrap();
        let first = table.iter().next().unwrap();
        assert_eq!(table.len(), 1);
        assert!(first.flags.read);
        assert!(first.flags.execute);
    }
}
