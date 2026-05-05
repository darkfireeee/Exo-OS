#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TlsImage {
    pub file_offset: u64,
    pub virt_addr: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
}
