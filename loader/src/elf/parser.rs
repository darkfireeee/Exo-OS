pub const EI_NIDENT: usize = 16;
pub const ELF_MAGIC: &[u8; 4] = b"\x7fELF";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    UnsupportedClass,
    UnsupportedEndian,
    UnsupportedVersion,
    UnsupportedMachine,
    BadProgramHeaderTable,
    ArithmeticOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfClass {
    Elf64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfEndian {
    Little,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfType {
    Executable,
    SharedObject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfMachine {
    X86_64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElfHeader {
    pub class: ElfClass,
    pub endian: ElfEndian,
    pub elf_type: ElfType,
    pub machine: ElfMachine,
    pub entry: u64,
    pub phoff: u64,
    pub phentsize: u16,
    pub phnum: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgramHeader {
    pub p_type: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_TLS: u32 = 7;

pub fn parse_header(image: &[u8]) -> Result<ElfHeader, ElfError> {
    if image.len() < 64 {
        return Err(ElfError::TooSmall);
    }
    if &image[0..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }
    if image[4] != 2 {
        return Err(ElfError::UnsupportedClass);
    }
    if image[5] != 1 {
        return Err(ElfError::UnsupportedEndian);
    }
    if image[6] != 1 {
        return Err(ElfError::UnsupportedVersion);
    }

    let elf_type = match read_u16(image, 16) {
        2 => ElfType::Executable,
        3 => ElfType::SharedObject,
        _ => return Err(ElfError::UnsupportedVersion),
    };
    let machine = match read_u16(image, 18) {
        62 => ElfMachine::X86_64,
        _ => return Err(ElfError::UnsupportedMachine),
    };
    if read_u32(image, 20) != 1 {
        return Err(ElfError::UnsupportedVersion);
    }

    let header = ElfHeader {
        class: ElfClass::Elf64,
        endian: ElfEndian::Little,
        elf_type,
        machine,
        entry: read_u64(image, 24),
        phoff: read_u64(image, 32),
        phentsize: read_u16(image, 54),
        phnum: read_u16(image, 56),
    };
    validate_program_header_span(image.len(), header)?;
    Ok(header)
}

pub fn program_header(
    image: &[u8],
    header: ElfHeader,
    index: u16,
) -> Result<ProgramHeader, ElfError> {
    if index >= header.phnum {
        return Err(ElfError::BadProgramHeaderTable);
    }
    if header.phentsize < 56 {
        return Err(ElfError::BadProgramHeaderTable);
    }
    let off = (header.phoff as usize)
        .checked_add(index as usize * header.phentsize as usize)
        .ok_or(ElfError::ArithmeticOverflow)?;
    let end = off.checked_add(56).ok_or(ElfError::ArithmeticOverflow)?;
    if end > image.len() {
        return Err(ElfError::BadProgramHeaderTable);
    }
    Ok(ProgramHeader {
        p_type: read_u32(image, off),
        flags: read_u32(image, off + 4),
        offset: read_u64(image, off + 8),
        vaddr: read_u64(image, off + 16),
        filesz: read_u64(image, off + 32),
        memsz: read_u64(image, off + 40),
        align: read_u64(image, off + 48),
    })
}

fn validate_program_header_span(image_len: usize, header: ElfHeader) -> Result<(), ElfError> {
    let phoff = header.phoff as usize;
    let phentsize = header.phentsize as usize;
    let phnum = header.phnum as usize;
    let table_bytes = phentsize
        .checked_mul(phnum)
        .ok_or(ElfError::ArithmeticOverflow)?;
    let end = phoff
        .checked_add(table_bytes)
        .ok_or(ElfError::ArithmeticOverflow)?;
    if phentsize != 0 && (phentsize < 56 || end > image_len) {
        return Err(ElfError::BadProgramHeaderTable);
    }
    Ok(())
}

#[inline]
fn read_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([data[off], data[off + 1]])
}

#[inline]
fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

#[inline]
fn read_u64(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        data[off],
        data[off + 1],
        data[off + 2],
        data[off + 3],
        data[off + 4],
        data[off + 5],
        data[off + 6],
        data[off + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_elf() -> [u8; 120] {
        let mut img = [0u8; 120];
        img[0..4].copy_from_slice(ELF_MAGIC);
        img[4] = 2;
        img[5] = 1;
        img[6] = 1;
        img[16..18].copy_from_slice(&2u16.to_le_bytes());
        img[18..20].copy_from_slice(&62u16.to_le_bytes());
        img[20..24].copy_from_slice(&1u32.to_le_bytes());
        img[24..32].copy_from_slice(&0x401000u64.to_le_bytes());
        img[32..40].copy_from_slice(&64u64.to_le_bytes());
        img[54..56].copy_from_slice(&56u16.to_le_bytes());
        img[56..58].copy_from_slice(&1u16.to_le_bytes());
        img[64..68].copy_from_slice(&PT_LOAD.to_le_bytes());
        img[68..72].copy_from_slice(&5u32.to_le_bytes());
        img[72..80].copy_from_slice(&0x100u64.to_le_bytes());
        img[80..88].copy_from_slice(&0x401000u64.to_le_bytes());
        img[96..104].copy_from_slice(&32u64.to_le_bytes());
        img[104..112].copy_from_slice(&64u64.to_le_bytes());
        img[112..120].copy_from_slice(&0x1000u64.to_le_bytes());
        img
    }

    #[test]
    fn parses_minimal_header_and_program_header() {
        let img = minimal_elf();
        let hdr = parse_header(&img).unwrap();
        assert_eq!(hdr.entry, 0x401000);
        let ph = program_header(&img, hdr, 0).unwrap();
        assert_eq!(ph.p_type, PT_LOAD);
        assert_eq!(ph.memsz, 64);
    }
}
