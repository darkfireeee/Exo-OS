const ETH_HEADER_LEN: usize = 14;
const ETHERTYPE_IPV4: u16 = 0x0800;
const IPV4_PROTO_ICMP: u8 = 1;
const IPV4_MIN_HEADER_LEN: usize = 20;
const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;

pub fn make_echo_reply_ipv4_frame(frame: &mut [u8]) -> Option<usize> {
    if frame.len() < ETH_HEADER_LEN + IPV4_MIN_HEADER_LEN + 8 {
        return None;
    }
    if u16::from_be_bytes([frame[12], frame[13]]) != ETHERTYPE_IPV4 {
        return None;
    }

    let ip_start = ETH_HEADER_LEN;
    if frame[ip_start] >> 4 != 4 {
        return None;
    }
    let ihl = ((frame[ip_start] & 0x0f) as usize) * 4;
    if ihl < IPV4_MIN_HEADER_LEN || frame.len() < ip_start + ihl + 8 {
        return None;
    }
    let total_len = u16::from_be_bytes([frame[ip_start + 2], frame[ip_start + 3]]) as usize;
    if total_len < ihl + 8 || frame.len() < ip_start + total_len {
        return None;
    }
    if frame[ip_start + 9] != IPV4_PROTO_ICMP {
        return None;
    }

    let icmp_start = ip_start + ihl;
    if frame[icmp_start] != ICMP_ECHO_REQUEST || frame[icmp_start + 1] != 0 {
        return None;
    }

    let mut mac = [0u8; 6];
    mac.copy_from_slice(&frame[0..6]);
    frame.copy_within(6..12, 0);
    frame[6..12].copy_from_slice(&mac);

    let mut ip = [0u8; 4];
    ip.copy_from_slice(&frame[ip_start + 12..ip_start + 16]);
    frame.copy_within(ip_start + 16..ip_start + 20, ip_start + 12);
    frame[ip_start + 16..ip_start + 20].copy_from_slice(&ip);
    frame[ip_start + 8] = 64;
    frame[ip_start + 10] = 0;
    frame[ip_start + 11] = 0;
    let ip_sum = checksum(&frame[ip_start..ip_start + ihl]);
    frame[ip_start + 10..ip_start + 12].copy_from_slice(&ip_sum.to_be_bytes());

    frame[icmp_start] = ICMP_ECHO_REPLY;
    frame[icmp_start + 2] = 0;
    frame[icmp_start + 3] = 0;
    let icmp_sum = checksum(&frame[icmp_start..ip_start + total_len]);
    frame[icmp_start + 2..icmp_start + 4].copy_from_slice(&icmp_sum.to_be_bytes());
    Some(ETH_HEADER_LEN + total_len)
}

pub fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut idx = 0usize;
    while idx + 1 < bytes.len() {
        sum = sum.wrapping_add(u16::from_be_bytes([bytes[idx], bytes[idx + 1]]) as u32);
        idx += 2;
    }
    if idx < bytes.len() {
        sum = sum.wrapping_add((bytes[idx] as u32) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
