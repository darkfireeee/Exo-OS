//! Network Stack - Comprehensive Tests
//!
//! Tests for all network components

use super::*;
use socket::{Socket, SocketAddr, SocketType, SocketDomain, Ipv4Addr, IpAddr};
use buffer::PacketBuffer;
use device::{NetworkDevice, LoopbackDevice};
use ethernet::{EthernetHeader, MacAddr};
use ip::{Ipv4Header, IcmpHeader, protocol};
use udp::UdpHeader;
use tcp::{TcpHeader, TcpConnection, TcpState, flags};
use arp::{ArpPacket, ArpCache, operation};

// ============================================================================
// SOCKET TESTS
// ============================================================================


fn test_socket_creation() {
    let socket = Socket::new(SocketDomain::Inet, SocketType::Stream);
    assert_eq!(socket.state(), socket::SocketState::Closed);
}


fn test_ipv4_addr_conversion() {
    let localhost = Ipv4Addr::localhost();
    assert_eq!(localhost.0, [127, 0, 0, 1]);
    
    let any = Ipv4Addr::any();
    assert_eq!(any.0, [0, 0, 0, 0]);
    
    let custom = Ipv4Addr::new(192, 168, 1, 1);
    assert_eq!(custom.0, [192, 168, 1, 1]);
}


fn test_socket_bind() {
    let mut socket = Socket::new(SocketDomain::Inet, SocketType::Stream);
    
    let addr = SocketAddr {
        ip: IpAddr::V4(Ipv4Addr::any()),
        port: 8080,
    };
    
    assert!(socket.bind(addr).is_ok());
    assert_eq!(socket.local_addr().unwrap().port, 8080);
}


fn test_socket_table() {
    let id = SOCKET_TABLE.create(SocketDomain::Inet, SocketType::Datagram);
    assert!(id > 0);
}

// ============================================================================
// BUFFER TESTS
// ============================================================================


fn test_packet_buffer_basic() {
    let mut pkt = PacketBuffer::new(256);
    
    // Reserve headroom for headers
    pkt.reserve_headroom(64);
    
    // Add payload
    assert!(pkt.put(b"Hello, Network!").is_ok());
    assert_eq!(pkt.len(), 15);
    
    // Check data
    assert_eq!(pkt.data(), b"Hello, Network!");
}


fn test_packet_buffer_push_pull() {
    let mut pkt = PacketBuffer::new(256);
    pkt.reserve_headroom(64);
    
    // Add payload
    pkt.put(b"Payload").unwrap();
    assert_eq!(pkt.len(), 7);
    
    // Add header (move data_ptr back)
    let header = pkt.push(14).unwrap();
    assert_eq!(header.len(), 14);
    assert_eq!(pkt.len(), 21); // 14 + 7
    
    // Remove header (move data_ptr forward)
    let removed = pkt.pull(14).unwrap();
    assert_eq!(removed.len(), 14);
    assert_eq!(pkt.len(), 7);
}


fn test_packet_buffer_headroom_tailroom() {
    let mut pkt = PacketBuffer::new(256);
    pkt.reserve_headroom(64);
    
    assert_eq!(pkt.headroom(), 64);
    assert_eq!(pkt.tailroom(), 192);
    
    pkt.put(b"Test").unwrap();
    assert_eq!(pkt.tailroom(), 188);
}


fn test_packet_buffer_pool() {
    // Allocate from pool
    let pkt1 = PACKET_POOL.alloc();
    assert_eq!(pkt1.len(), 0);
    
    let pkt2 = PACKET_POOL.alloc();
    assert_eq!(pkt2.len(), 0);
    
    // Return to pool
    PACKET_POOL.free(pkt1);
    PACKET_POOL.free(pkt2);
}

// ============================================================================
// DEVICE TESTS
// ============================================================================


fn test_loopback_device() {
    let mut lo = LoopbackDevice::new();
    assert_eq!(lo.name(), "lo");
    assert_eq!(lo.mtu(), 65536);
    assert!(!lo.is_up());
    
    lo.up().unwrap();
    assert!(lo.is_up());
}


fn test_loopback_echo() {
    let mut lo = LoopbackDevice::new();
    lo.up().unwrap();
    
    // Send packet
    let mut pkt = PacketBuffer::with_default_capacity();
    pkt.put(b"Echo test").unwrap();
    lo.transmit(pkt).unwrap();
    
    // Receive it back
    let rx_pkt = lo.receive().unwrap();
    assert!(rx_pkt.is_some());
    
    let stats = lo.stats();
    assert_eq!(stats.tx_packets, 1);
    assert_eq!(stats.rx_packets, 1);
}


fn test_device_stats() {
    let mut lo = LoopbackDevice::new();
    lo.up().unwrap();
    
    for i in 0..10 {
        let mut pkt = PacketBuffer::with_default_capacity();
        pkt.put(&[0u8; 100]).unwrap();
        lo.transmit(pkt).unwrap();
    }
    
    let stats = lo.stats();
    assert_eq!(stats.tx_packets, 10);
    assert_eq!(stats.rx_packets, 10);
    assert_eq!(stats.tx_bytes, 1000);
}

// ============================================================================
// ETHERNET TESTS
// ============================================================================


fn test_ethernet_header() {
    let header = EthernetHeader::new(
        [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], // Broadcast
        [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
        ethernet::ether_type::IPV4,
    );
    
    assert!(header.is_broadcast());
    assert_eq!(header.protocol(), ethernet::ether_type::IPV4);
}


fn test_ethernet_parse_write() {
    let original = EthernetHeader::new(
        [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
        [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
        ethernet::ether_type::ARP,
    );
    
    let mut buffer = [0u8; 14];
    original.write(&mut buffer).unwrap();
    
    let parsed = EthernetHeader::parse(&buffer).unwrap();
    assert_eq!(parsed.dst_mac, original.dst_mac);
    assert_eq!(parsed.src_mac, original.src_mac);
    assert_eq!(parsed.protocol(), ethernet::ether_type::ARP);
}


fn test_mac_addr() {
    let broadcast = MacAddr::broadcast();
    assert!(broadcast.is_broadcast());
    assert!(!broadcast.is_unicast());
    
    let unicast = MacAddr([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    assert!(unicast.is_unicast());
    assert!(!unicast.is_broadcast());
    
    let multicast = MacAddr([0x01, 0x00, 0x5E, 0x00, 0x00, 0x01]);
    assert!(multicast.is_multicast());
}

// ============================================================================
// IPv4 TESTS
// ============================================================================


fn test_ipv4_header() {
    let src = Ipv4Addr::new(192, 168, 1, 1);
    let dst = Ipv4Addr::new(8, 8, 8, 8);
    
    let header = Ipv4Header::new(src, dst, protocol::TCP, 100);
    
    assert_eq!(header.version(), 4);
    assert_eq!(header.header_len(), 20);
    assert_eq!(header.total_len(), 120);
    assert_eq!(header.payload_len(), 100);
    assert!(header.verify_checksum());
}


fn test_ipv4_parse_write() {
    let src = Ipv4Addr::new(10, 0, 0, 1);
    let dst = Ipv4Addr::new(10, 0, 0, 2);
    
    let original = Ipv4Header::new(src, dst, protocol::UDP, 200);
    
    let mut buffer = [0u8; 20];
    original.write(&mut buffer).unwrap();
    
    let parsed = Ipv4Header::parse(&buffer).unwrap();
    assert_eq!(parsed.src().0, src.0);
    assert_eq!(parsed.dst().0, dst.0);
    assert_eq!(parsed.protocol, protocol::UDP);
    assert!(parsed.verify_checksum());
}


fn test_icmp_echo() {
    let icmp = IcmpHeader::echo_request(1234, 1);
    
    assert_eq!(icmp.icmp_type, ip::icmp::ECHO_REQUEST);
    assert_eq!(u16::from_be(icmp.identifier), 1234);
    assert_eq!(u16::from_be(icmp.sequence), 1);
}

// ============================================================================
// UDP TESTS
// ============================================================================


fn test_udp_header() {
    let header = UdpHeader::new(1234, 5678, 100);
    
    assert_eq!(header.src_port(), 1234);
    assert_eq!(header.dst_port(), 5678);
    assert_eq!(header.length(), 108); // 8 + 100
    assert_eq!(header.payload_len(), 100);
}


fn test_udp_parse_write() {
    let original = UdpHeader::new(53, 12345, 50);
    
    let mut buffer = [0u8; 8];
    original.write(&mut buffer).unwrap();
    
    let parsed = UdpHeader::parse(&buffer).unwrap();
    assert_eq!(parsed.src_port(), 53);
    assert_eq!(parsed.dst_port(), 12345);
    assert_eq!(parsed.length(), 58);
}


fn test_udp_socket() {
    let mut socket = udp::UdpSocket::new();
    
    socket.bind(1234).unwrap();
    
    let addr = Ipv4Addr::new(127, 0, 0, 1);
    socket.connect(addr, 5678).unwrap();
    
    // Send would work if network layer was connected
    let data = b"UDP test";
    let result = socket.send(data);
    assert!(result.is_ok());
}

// ============================================================================
// TCP TESTS
// ============================================================================


fn test_tcp_header() {
    let header = TcpHeader::new(1234, 80, 1000, 2000, flags::SYN, 65535);
    
    assert_eq!(header.src_port(), 1234);
    assert_eq!(header.dst_port(), 80);
    assert_eq!(header.seq(), 1000);
    assert_eq!(header.ack(), 2000);
    assert!(header.has_flag(flags::SYN));
    assert_eq!(header.header_len(), 20);
}


fn test_tcp_flags() {
    let syn = TcpHeader::new(1234, 80, 0, 0, flags::SYN, 65535);
    assert!(syn.has_flag(flags::SYN));
    assert!(!syn.has_flag(flags::ACK));
    
    let syn_ack = TcpHeader::new(80, 1234, 0, 0, flags::SYN | flags::ACK, 65535);
    assert!(syn_ack.has_flag(flags::SYN));
    assert!(syn_ack.has_flag(flags::ACK));
    
    let fin = TcpHeader::new(1234, 80, 0, 0, flags::FIN | flags::ACK, 65535);
    assert!(fin.has_flag(flags::FIN));
    assert!(fin.has_flag(flags::ACK));
}


fn test_tcp_state_machine_connect() {
    let local = Ipv4Addr::new(192, 168, 1, 1);
    let remote = Ipv4Addr::new(192, 168, 1, 2);
    
    let mut conn = TcpConnection::new(local, 1234, remote, 80);
    
    assert_eq!(conn.state(), TcpState::Closed);
    
    // Active open (client)
    conn.connect().unwrap();
    assert_eq!(conn.state(), TcpState::SynSent);
}


fn test_tcp_state_machine_listen() {
    let local = Ipv4Addr::new(192, 168, 1, 1);
    let remote = Ipv4Addr::new(0, 0, 0, 0);
    
    let mut conn = TcpConnection::new(local, 80, remote, 0);
    
    // Passive open (server)
    conn.listen().unwrap();
    assert_eq!(conn.state(), TcpState::Listen);
}


fn test_tcp_3way_handshake() {
    let local = Ipv4Addr::new(192, 168, 1, 1);
    let remote = Ipv4Addr::new(192, 168, 1, 2);
    
    let mut client = TcpConnection::new(local, 1234, remote, 80);
    let mut server = TcpConnection::new(remote, 80, local, 1234);
    
    // Server listens
    server.listen().unwrap();
    assert_eq!(server.state(), TcpState::Listen);
    
    // Client sends SYN
    client.connect().unwrap();
    assert_eq!(client.state(), TcpState::SynSent);
    
    // Server receives SYN (simulated)
    let syn = TcpHeader::new(1234, 80, client.snd_nxt() - 1, 0, flags::SYN, 65535);
    server.handle_packet(&syn, &[]).unwrap();
    assert_eq!(server.state(), TcpState::SynReceived);
    
    // Client receives SYN-ACK (simulated)
    let syn_ack = TcpHeader::new(80, 1234, server.snd_nxt() - 1, client.snd_nxt(), flags::SYN | flags::ACK, 65535);
    client.handle_packet(&syn_ack, &[]).unwrap();
    assert_eq!(client.state(), TcpState::Established);
}


fn test_tcp_close() {
    let local = Ipv4Addr::new(192, 168, 1, 1);
    let remote = Ipv4Addr::new(192, 168, 1, 2);
    
    let mut conn = TcpConnection::new(local, 1234, remote, 80);
    
    // Establish connection first (simplified)
    conn.connect().unwrap();
    
    // Force to established state for test
    let syn_ack = TcpHeader::new(80, 1234, 1000, conn.snd_nxt(), flags::SYN | flags::ACK, 65535);
    conn.handle_packet(&syn_ack, &[]).unwrap();
    assert_eq!(conn.state(), TcpState::Established);
    
    // Close connection
    conn.close().unwrap();
    assert_eq!(conn.state(), TcpState::FinWait1);
}

// ============================================================================
// ARP TESTS
// ============================================================================


fn test_arp_request() {
    let sender_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let sender_ip = Ipv4Addr::new(192, 168, 1, 1);
    let target_ip = Ipv4Addr::new(192, 168, 1, 2);
    
    let packet = ArpPacket::request(sender_mac, sender_ip, target_ip);
    
    assert_eq!(packet.operation(), operation::REQUEST);
    assert_eq!(packet.sender_ip().0, sender_ip.0);
    assert_eq!(packet.target_ip().0, target_ip.0);
}


fn test_arp_reply() {
    let sender_mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    let sender_ip = Ipv4Addr::new(192, 168, 1, 2);
    let target_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let target_ip = Ipv4Addr::new(192, 168, 1, 1);
    
    let packet = ArpPacket::reply(sender_mac, sender_ip, target_mac, target_ip);
    
    assert_eq!(packet.operation(), operation::REPLY);
    assert_eq!(packet.sender_ip().0, sender_ip.0);
    assert_eq!(packet.target_ip().0, target_ip.0);
}


fn test_arp_cache_basic() {
    let mut cache = ArpCache::new();
    cache.init(10);
    
    let ip = Ipv4Addr::new(192, 168, 1, 1);
    let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    
    // Insert
    cache.insert(ip, mac);
    assert_eq!(cache.len(), 1);
    
    // Lookup
    let found = cache.lookup(ip);
    assert_eq!(found, Some(mac));
    
    // Remove
    cache.remove(ip);
    assert_eq!(cache.len(), 0);
}


fn test_arp_cache_update() {
    let mut cache = ArpCache::new();
    cache.init(10);
    
    let ip = Ipv4Addr::new(192, 168, 1, 1);
    let mac1 = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let mac2 = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    
    // Insert first MAC
    cache.insert(ip, mac1);
    assert_eq!(cache.lookup(ip), Some(mac1));
    
    // Update with second MAC
    cache.insert(ip, mac2);
    assert_eq!(cache.len(), 1); // Still 1 entry
    assert_eq!(cache.lookup(ip), Some(mac2)); // Updated
}


fn test_arp_cache_eviction() {
    let mut cache = ArpCache::new();
    cache.init(3); // Small capacity
    
    // Fill cache
    for i in 1..=3 {
        let ip = Ipv4Addr::new(192, 168, 1, i);
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, i as u8];
        cache.insert(ip, mac);
    }
    assert_eq!(cache.len(), 3);
    
    // Add one more (should evict oldest)
    let ip4 = Ipv4Addr::new(192, 168, 1, 4);
    let mac4 = [0x00, 0x11, 0x22, 0x33, 0x44, 0x04];
    cache.insert(ip4, mac4);
    
    assert_eq!(cache.len(), 3); // Still 3
    
    // First entry should be evicted
    let ip1 = Ipv4Addr::new(192, 168, 1, 1);
    assert_eq!(cache.lookup(ip1), None);
}

// ============================================================================
// INTEGRATION TESTS
// ============================================================================


fn test_full_udp_packet() {
    let mut pkt = PacketBuffer::new(512);
    pkt.reserve_headroom(64);
    
    // Add payload
    let payload = b"UDP test payload";
    pkt.put(payload).unwrap();
    
    // Add UDP header
    let udp_space = pkt.push(8).unwrap();
    let udp = UdpHeader::new(1234, 5678, payload.len() as u16);
    udp.write(udp_space).unwrap();
    
    // Add IPv4 header
    let ip_space = pkt.push(20).unwrap();
    let src = Ipv4Addr::new(192, 168, 1, 1);
    let dst = Ipv4Addr::new(192, 168, 1, 2);
    let ip = Ipv4Header::new(src, dst, protocol::UDP, (8 + payload.len()) as u16);
    ip.write(ip_space).unwrap();
    
    // Add Ethernet header
    let eth_space = pkt.push(14).unwrap();
    let eth = EthernetHeader::new(
        [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
        [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
        ethernet::ether_type::IPV4,
    );
    eth.write(eth_space).unwrap();
    
    // Total packet should be: 14 (eth) + 20 (ip) + 8 (udp) + 16 (payload) = 58
    assert_eq!(pkt.len(), 58);
}


fn test_loopback_udp_echo() {
    let mut lo = LoopbackDevice::new();
    lo.up().unwrap();
    
    // Create UDP packet
    let mut pkt = PacketBuffer::new(512);
    pkt.reserve_headroom(64);
    
    pkt.put(b"Echo").unwrap();
    
    // Transmit
    lo.transmit(pkt).unwrap();
    
    // Receive
    let rx = lo.receive().unwrap();
    assert!(rx.is_some());
    
    let rx_pkt = rx.unwrap();
    assert_eq!(rx_pkt.data(), b"Echo");
}

// ============================================================================
// PERFORMANCE TESTS
// ============================================================================


fn test_buffer_pool_performance() {
    // Allocate 100 buffers
    let mut buffers = alloc::vec::Vec::new();
    for _ in 0..100 {
        buffers.push(PACKET_POOL.alloc());
    }
    
    // Free them all
    for buf in buffers {
        PACKET_POOL.free(buf);
    }
}


fn test_arp_cache_performance() {
    let mut cache = ArpCache::new();
    cache.init(256);
    
    // Insert 100 entries
    for i in 0..100 {
        let ip = Ipv4Addr::new(192, 168, (i / 256) as u8, (i % 256) as u8);
        let mac = [0x00, 0x11, 0x22, 0x33, (i / 256) as u8, (i % 256) as u8];
        cache.insert(ip, mac);
    }
    
    assert_eq!(cache.len(), 100);
    
    // Lookup all entries
    for i in 0..100 {
        let ip = Ipv4Addr::new(192, 168, (i / 256) as u8, (i % 256) as u8);
        assert!(cache.lookup(ip).is_some());
    }
}

// ============================================================================
// TEST RUNNER
// ============================================================================

pub fn run_all_network_tests() -> (usize, usize) {
    let mut passed = 0;
    let mut total = 0;
    
    macro_rules! run_test {
        ($test:ident, $name:expr) => {
            total += 1;
            crate::logger::info(&alloc::format!("  Running: {}", $name));
            $test();
            crate::logger::info(&alloc::format!("  ✅ {}", $name));
            passed += 1;
        };
    }
    
    crate::logger::info("[NET TESTS] Running network stack tests...");
    
    crate::logger::info("[NET TESTS] Socket tests:");
    run_test!(test_socket_creation, "Socket creation");
    run_test!(test_ipv4_addr_conversion, "IPv4 address conversion");
    run_test!(test_socket_bind, "Socket bind");
    run_test!(test_socket_table, "Socket table");
    
    crate::logger::info("[NET TESTS] Buffer tests:");
    run_test!(test_packet_buffer_basic, "Packet buffer basic");
    run_test!(test_packet_buffer_push_pull, "Packet buffer push/pull");
    run_test!(test_packet_buffer_headroom_tailroom, "Packet buffer headroom/tailroom");
    run_test!(test_packet_buffer_pool, "Packet buffer pool");
    
    crate::logger::info("[NET TESTS] Device tests:");
    run_test!(test_loopback_device, "Loopback device");
    run_test!(test_loopback_echo, "Loopback echo");
    run_test!(test_device_stats, "Device statistics");
    
    crate::logger::info("[NET TESTS] Ethernet tests:");
    run_test!(test_ethernet_header, "Ethernet header");
    run_test!(test_ethernet_parse_write, "Ethernet parse/write");
    run_test!(test_mac_addr, "MAC address");
    
    crate::logger::info("[NET TESTS] IPv4 tests:");
    run_test!(test_ipv4_header, "IPv4 header");
    run_test!(test_ipv4_parse_write, "IPv4 parse/write");
    run_test!(test_icmp_echo, "ICMP echo");
    
    crate::logger::info("[NET TESTS] UDP tests:");
    run_test!(test_udp_header, "UDP header");
    run_test!(test_udp_parse_write, "UDP parse/write");
    run_test!(test_udp_socket, "UDP socket");
    
    crate::logger::info("[NET TESTS] TCP tests:");
    run_test!(test_tcp_header, "TCP header");
    run_test!(test_tcp_flags, "TCP flags");
    run_test!(test_tcp_state_machine_connect, "TCP state machine - connect");
    run_test!(test_tcp_state_machine_listen, "TCP state machine - listen");
    run_test!(test_tcp_3way_handshake, "TCP 3-way handshake");
    run_test!(test_tcp_close, "TCP close");
    
    crate::logger::info("[NET TESTS] ARP tests:");
    run_test!(test_arp_request, "ARP request");
    run_test!(test_arp_reply, "ARP reply");
    run_test!(test_arp_cache_basic, "ARP cache basic");
    run_test!(test_arp_cache_update, "ARP cache update");
    run_test!(test_arp_cache_eviction, "ARP cache eviction");
    
    crate::logger::info("[NET TESTS] Integration tests:");
    run_test!(test_full_udp_packet, "Full UDP packet");
    run_test!(test_loopback_udp_echo, "Loopback UDP echo");
    
    crate::logger::info("[NET TESTS] Performance tests:");
    run_test!(test_buffer_pool_performance, "Buffer pool performance");
    run_test!(test_arp_cache_performance, "ARP cache performance");
    
    crate::logger::info(&alloc::format!(
        "\n[NET TESTS] Results: {}/{} tests passed ({}%)",
        passed,
        total,
        (passed * 100) / total.max(1)
    ));
    
    (passed, total)
}
