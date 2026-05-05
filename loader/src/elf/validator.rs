use super::parser::{parse_header, ElfError};
use super::segments::SegmentTable;

pub fn validate_static_executable(image: &[u8]) -> Result<(), ElfError> {
    let header = parse_header(image)?;
    let segments = SegmentTable::<16>::parse(image, header)?;
    if segments.len() == 0 {
        return Err(ElfError::BadProgramHeaderTable);
    }
    for seg in segments.iter() {
        if seg.mem_size == 0 || seg.align > 0x20_0000 {
            return Err(ElfError::BadProgramHeaderTable);
        }
    }
    Ok(())
}
