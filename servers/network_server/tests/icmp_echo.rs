#[path = "../src/icmp.rs"]
mod icmp;

#[test]
fn echo_request_is_rewritten_as_reply() {
    let mut frame = [0u8; 64];
    frame[0..6].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    frame[6..12].copy_from_slice(&[6, 5, 4, 3, 2, 1]);
    frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    frame[14] = 0x45;
    frame[16..18].copy_from_slice(&28_u16.to_be_bytes());
    frame[22] = 64;
    frame[23] = 1;
    frame[26..30].copy_from_slice(&0x0a00_020f_u32.to_be_bytes());
    frame[30..34].copy_from_slice(&0x0a00_0202_u32.to_be_bytes());
    let ip_sum = icmp::checksum(&frame[14..34]);
    frame[24..26].copy_from_slice(&ip_sum.to_be_bytes());
    frame[34] = 8;
    frame[38..42].copy_from_slice(&0x1234_0001_u32.to_be_bytes());
    let icmp_sum = icmp::checksum(&frame[34..42]);
    frame[36..38].copy_from_slice(&icmp_sum.to_be_bytes());

    assert_eq!(icmp::make_echo_reply_ipv4_frame(&mut frame), Some(42));
    assert_eq!(&frame[0..6], &[6, 5, 4, 3, 2, 1]);
    assert_eq!(&frame[6..12], &[1, 2, 3, 4, 5, 6]);
    assert_eq!(frame[34], 0);
    assert_eq!(&frame[26..30], &0x0a00_0202_u32.to_be_bytes());
    assert_eq!(&frame[30..34], &0x0a00_020f_u32.to_be_bytes());
}
