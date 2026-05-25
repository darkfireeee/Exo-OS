pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const ETHER_HEADER_LEN: usize = 14;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EthernetHeader {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
}

impl EthernetHeader {
    pub fn parse(frame: &[u8]) -> Option<Self> {
        if frame.len() < ETHER_HEADER_LEN {
            return None;
        }
        let mut dst = [0u8; 6];
        let mut src = [0u8; 6];
        dst.copy_from_slice(&frame[0..6]);
        src.copy_from_slice(&frame[6..12]);
        Some(Self {
            dst,
            src,
            ethertype: u16::from_be_bytes([frame[12], frame[13]]),
        })
    }

    pub fn write(&self, frame: &mut [u8]) -> Option<()> {
        if frame.len() < ETHER_HEADER_LEN {
            return None;
        }
        frame[0..6].copy_from_slice(&self.dst);
        frame[6..12].copy_from_slice(&self.src);
        frame[12..14].copy_from_slice(&self.ethertype.to_be_bytes());
        Some(())
    }
}
