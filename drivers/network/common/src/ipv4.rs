pub const IPV4_MIN_HEADER_LEN: usize = 20;
pub const IPV4_PROTO_ICMP: u8 = 1;
pub const IPV4_PROTO_TCP: u8 = 6;
pub const IPV4_PROTO_UDP: u8 = 17;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4Header {
    pub header_len: usize,
    pub total_len: u16,
    pub protocol: u8,
    pub src: u32,
    pub dst: u32,
}

impl Ipv4Header {
    pub fn parse(packet: &[u8]) -> Option<Self> {
        if packet.len() < IPV4_MIN_HEADER_LEN || packet[0] >> 4 != 4 {
            return None;
        }
        let ihl = ((packet[0] & 0x0f) as usize) * 4;
        if ihl < IPV4_MIN_HEADER_LEN || packet.len() < ihl {
            return None;
        }
        let total_len = u16::from_be_bytes([packet[2], packet[3]]);
        if total_len as usize > packet.len() || (total_len as usize) < ihl {
            return None;
        }
        Some(Self {
            header_len: ihl,
            total_len,
            protocol: packet[9],
            src: u32::from_be_bytes([packet[12], packet[13], packet[14], packet[15]]),
            dst: u32::from_be_bytes([packet[16], packet[17], packet[18], packet[19]]),
        })
    }
}

pub fn checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0usize;
    while i + 1 < header.len() {
        sum = sum.wrapping_add(u16::from_be_bytes([header[i], header[i + 1]]) as u32);
        i += 2;
    }
    if i < header.len() {
        sum = sum.wrapping_add((header[i] as u32) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
