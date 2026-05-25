pub fn is_loopback_ipv4(ip: u32) -> bool {
    (ip & 0xff00_0000) == 0x7f00_0000
}

pub fn swap_ipv4_endpoints(packet: &mut [u8]) -> bool {
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return false;
    }
    let ihl = ((packet[0] & 0x0f) as usize) * 4;
    if ihl < 20 || packet.len() < ihl {
        return false;
    }
    let src = [packet[12], packet[13], packet[14], packet[15]];
    let dst = [packet[16], packet[17], packet[18], packet[19]];
    let dst_ip = u32::from_be_bytes(dst);
    if !is_loopback_ipv4(dst_ip) {
        return false;
    }
    packet[12..16].copy_from_slice(&dst);
    packet[16..20].copy_from_slice(&src);
    packet[10] = 0;
    packet[11] = 0;
    let csum = ipv4_checksum(&packet[..ihl]);
    packet[10..12].copy_from_slice(&csum.to_be_bytes());
    true
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0usize;
    while i + 1 < header.len() {
        sum = sum.wrapping_add(u16::from_be_bytes([header[i], header[i + 1]]) as u32);
        i += 2;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
