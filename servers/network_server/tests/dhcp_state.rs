#[path = "../src/dhcp.rs"]
mod dhcp;

use dhcp::{DhcpAction, DhcpClient, DhcpPhase, DHCP_FIXED_LEN, DHCP_MAGIC_COOKIE};

#[test]
fn dhcp_discover_offer_request_ack_reaches_bound() {
    let mac = [0x02, 0x45, 0x58, 0x4f, 0, 1];
    let mut client = DhcpClient::new();
    client.configure_mac(mac);
    client.start(0x1234_5678);

    let mut discover = [0u8; 320];
    assert_eq!(client.poll(1), DhcpAction::Discover);
    assert!(client.build_discover(&mut discover).unwrap() > DHCP_FIXED_LEN);

    let xid = u32::from_be_bytes([discover[4], discover[5], discover[6], discover[7]]);
    let offer = make_reply(xid, mac, 2);
    assert_eq!(client.ingest(&offer, 10), None);
    assert_eq!(client.state(), DhcpPhase::Requesting);

    let mut request = [0u8; 320];
    assert_eq!(client.poll(11), DhcpAction::Request);
    assert!(client.build_request(&mut request).unwrap() > DHCP_FIXED_LEN);

    let ack = make_reply(xid, mac, 5);
    let lease = client.ingest(&ack, 12).expect("ack produces lease");
    assert_eq!(client.state(), DhcpPhase::Bound);
    assert_eq!(lease.ip, 0x0a00_020f);
    assert_eq!(lease.gateway, 0x0a00_0202);
    assert_eq!(lease.prefix_len, 24);
}

fn make_reply(xid: u32, mac: [u8; 6], msg_type: u8) -> [u8; 320] {
    let mut packet = [0u8; 320];
    packet[0] = 2;
    packet[1] = 1;
    packet[2] = 6;
    packet[4..8].copy_from_slice(&xid.to_be_bytes());
    packet[16..20].copy_from_slice(&0x0a00_020f_u32.to_be_bytes());
    packet[28..34].copy_from_slice(&mac);
    packet[DHCP_FIXED_LEN..DHCP_FIXED_LEN + 4].copy_from_slice(&DHCP_MAGIC_COOKIE);
    let mut pos = DHCP_FIXED_LEN + 4;
    push(&mut packet, &mut pos, 53, &[msg_type]);
    push(&mut packet, &mut pos, 54, &0x0a00_0202_u32.to_be_bytes());
    push(&mut packet, &mut pos, 1, &0xffff_ff00_u32.to_be_bytes());
    push(&mut packet, &mut pos, 3, &0x0a00_0202_u32.to_be_bytes());
    push(&mut packet, &mut pos, 6, &0x0101_0101_u32.to_be_bytes());
    push(&mut packet, &mut pos, 51, &3600_u32.to_be_bytes());
    packet[pos] = 255;
    packet
}

fn push(packet: &mut [u8], pos: &mut usize, opt: u8, data: &[u8]) {
    packet[*pos] = opt;
    packet[*pos + 1] = data.len() as u8;
    packet[*pos + 2..*pos + 2 + data.len()].copy_from_slice(data);
    *pos += 2 + data.len();
}
