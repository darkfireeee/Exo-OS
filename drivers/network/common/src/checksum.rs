//! checksum.rs — Calcul de checksums réseau logiciels et drapeaux d'offload
//!
//! Conforme RFC 1071, RFC 793 (TCP), RFC 768 (UDP).
//! Règle SRV-02 : arithmétique entière pure (u32/u16) sans flottant ni SIMD.

pub const VIRTIO_NET_F_CSUM: u64 = 1 << 0;
pub const VIRTIO_NET_F_GUEST_CSUM: u64 = 1 << 1;

pub const NET_CSUM_OFFLOAD_NONE: u32 = 0;
pub const NET_CSUM_OFFLOAD_TCP: u32 = 1 << 0;
pub const NET_CSUM_OFFLOAD_UDP: u32 = 1 << 1;
pub const NET_CSUM_OFFLOAD_IP: u32 = 1 << 2;

/// Somme RFC 1071 sur un slice d'octets avec accumulation 16-bit
pub fn rfc1071_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);

    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]);
        sum = sum.wrapping_add(word as u32);
    }

    if let [b] = chunks.remainder() {
        let word = u16::from_be_bytes([*b, 0]);
        sum = sum.wrapping_add(word as u32);
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xffff).wrapping_add(sum >> 16);
    }

    !(sum as u16)
}

/// Calcule la somme du pseudo-header IPv4 pour TCP ou UDP
pub fn ipv4_pseudo_header_sum(src_ip: &[u8; 4], dst_ip: &[u8; 4], protocol: u8, length: u16) -> u32 {
    let mut sum: u32 = 0;

    let src_high = u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32;
    let src_low = u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32;
    let dst_high = u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32;
    let dst_low = u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32;

    sum = sum.wrapping_add(src_high);
    sum = sum.wrapping_add(src_low);
    sum = sum.wrapping_add(dst_high);
    sum = sum.wrapping_add(dst_low);
    sum = sum.wrapping_add(protocol as u32);
    sum = sum.wrapping_add(length as u32);

    sum
}

/// Calcule le checksum TCP complet (pseudo-header + payload TCP)
pub fn tcp_checksum(src_ip: &[u8; 4], dst_ip: &[u8; 4], tcp_segment: &[u8]) -> u16 {
    let mut sum = ipv4_pseudo_header_sum(src_ip, dst_ip, 6, tcp_segment.len() as u16);

    let mut chunks = tcp_segment.chunks_exact(2);
    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]);
        sum = sum.wrapping_add(word as u32);
    }

    if let [b] = chunks.remainder() {
        let word = u16::from_be_bytes([*b, 0]);
        sum = sum.wrapping_add(word as u32);
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xffff).wrapping_add(sum >> 16);
    }

    !(sum as u16)
}

/// Calcule le checksum UDP complet (pseudo-header + datagramme UDP)
pub fn udp_checksum(src_ip: &[u8; 4], dst_ip: &[u8; 4], udp_datagram: &[u8]) -> u16 {
    let mut sum = ipv4_pseudo_header_sum(src_ip, dst_ip, 17, udp_datagram.len() as u16);

    let mut chunks = udp_datagram.chunks_exact(2);
    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]);
        sum = sum.wrapping_add(word as u32);
    }

    if let [b] = chunks.remainder() {
        let word = u16::from_be_bytes([*b, 0]);
        sum = sum.wrapping_add(word as u32);
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xffff).wrapping_add(sum >> 16);
    }

    let csum = !(sum as u16);
    // En UDP, si le checksum calculé vaut 0, on transmet 0xFFFF (0 signifie pas de checksum)
    if csum == 0 {
        0xffff
    } else {
        csum
    }
}
