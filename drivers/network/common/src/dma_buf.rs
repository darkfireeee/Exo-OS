#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DmaBuffer {
    pub phys_addr: u64,
    pub virt_addr: u64,
    pub size: usize,
}

impl DmaBuffer {
    pub const fn new(phys_addr: u64, virt_addr: u64, size: usize) -> Self {
        Self {
            phys_addr,
            virt_addr,
            size,
        }
    }

    pub const fn is_page_aligned(&self) -> bool {
        (self.phys_addr & 0xfff) == 0 && (self.virt_addr & 0xfff) == 0
    }

    pub const fn contains_offset(&self, offset: usize, len: usize) -> bool {
        match offset.checked_add(len) {
            Some(end) => end <= self.size,
            None => false,
        }
    }
}
