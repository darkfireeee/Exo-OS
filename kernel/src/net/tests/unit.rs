//! Unit Tests - Component-level Testing
//!
//! Tests for individual network components in isolation.

#[cfg(test)]
mod tests {
    use crate::net::*;
    
    // ============================================================================
    // IP Address Tests
    // ============================================================================
    
    #[test]
    fn test_ipv4_address_creation() {
        use ip::Ipv4Address;
        
        let addr = Ipv4Address([192, 168, 1, 1]);
        assert_eq!(addr.0, [192, 168, 1, 1]);
        assert_eq!(addr.is_private(), true);
    }
    
    #[test]
    fn test_ipv6_address_creation() {
        use ip::Ipv6Address;
        
        let loopback = Ipv6Address([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(loopback.is_loopback(), true);
    }
    
    #[test]
    fn test_ipv4_network_contains() {
        use ip::routing::IpNetwork;
        use ip::routing::IpAddr;
        
        let network = IpNetwork {
            addr: IpAddr::V4([192, 168, 1, 0]),
            prefix_len: 24,
        };
        
        assert!(network.contains(&IpAddr::V4([192, 168, 1, 1])));
        assert!(network.contains(&IpAddr::V4([192, 168, 1, 254])));
        assert!(!network.contains(&IpAddr::V4([192, 168, 2, 1])));
    }
    
    // ============================================================================
    // MAC Address Tests
    // ============================================================================
    
    #[test]
    fn test_mac_address_broadcast() {
        use ethernet::MacAddress;
        
        let broadcast = MacAddress::BROADCAST;
        assert_eq!(broadcast.0, [0xFF; 6]);
        assert!(broadcast.is_broadcast());
    }
    
    #[test]
    fn test_mac_address_multicast() {
        use ethernet::MacAddress;
        
        let multicast = MacAddress([0x01, 0x00, 0x5E, 0x00, 0x00, 0x01]);
        assert!(multicast.is_multicast());
    }
    
    // ============================================================================
    // TCP Segment Tests
    // ============================================================================
    
    #[test]
    fn test_tcp_segment_creation() {
        use tcp::segment::TcpSegment;
        use tcp::segment::TcpFlags;
        
        let segment = TcpSegment {
            src_port: 80,
            dst_port: 12345,
            seq_number: 1000,
            ack_number: 2000,
            flags: TcpFlags::SYN,
            window_size: 65535,
            urgent_pointer: 0,
            options: alloc::vec::Vec::new(),
            payload: alloc::vec::Vec::new(),
        };
        
        assert_eq!(segment.src_port, 80);
        assert_eq!(segment.flags, TcpFlags::SYN);
    }
    
    // ============================================================================
    // UDP Tests
    // ============================================================================
    
    #[test]
    fn test_udp_header_creation() {
        use protocols::udp::UdpHeader;
        
        let header = UdpHeader::new(53, 12345, 100);
        assert_eq!(header.src_port(), 53);
        assert_eq!(header.dst_port(), 12345);
        assert_eq!(header.length(), 100);
    }
    
    // ============================================================================
    // Routing Tests
    // ============================================================================
    
    #[test]
    fn test_routing_table_add_route() {
        use ip::routing::{RoutingTable, Route, IpNetwork, IpAddr, RouteSource};
        
        let table = RoutingTable::new();
        
        let route = Route {
            destination: IpNetwork {
                addr: IpAddr::V4([192, 168, 1, 0]),
                prefix_len: 24,
            },
            gateway: Some(IpAddr::V4([192, 168, 1, 1])),
            interface: 0,
            metric: 100,
            source: RouteSource::Static,
        };
        
        table.add_route(route);
        
        let next_hop = table.lookup(&IpAddr::V4([192, 168, 1, 100]));
        assert!(next_hop.is_some());
    }
    
    // ============================================================================
    // Checksum Tests
    // ============================================================================
    
    #[test]
    fn test_ip_checksum() {
        use ip::ipv4::checksum;
        
        let data = [0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00,
                    0x40, 0x06, 0x00, 0x00, 0xac, 0x10, 0x0a, 0x63,
                    0xac, 0x10, 0x0a, 0x0c];
        
        let sum = checksum(&data);
        assert_ne!(sum, 0); // Non-zero checksum
    }
    
    // ============================================================================
    // Packet Buffer Tests
    // ============================================================================
    
    #[test]
    fn test_packet_buffer_allocation() {
        use core::buffer::PacketBuffer;
        
        let buffer = PacketBuffer::new(1500);
        assert_eq!(buffer.capacity(), 1500);
        assert_eq!(buffer.len(), 0);
    }
    
    #[test]
    fn test_packet_buffer_headroom() {
        use core::buffer::PacketBuffer;
        
        let mut buffer = PacketBuffer::with_headroom(128, 1500);
        assert_eq!(buffer.headroom(), 128);
        
        // Push header
        buffer.push_header(&[0xFF; 14]); // Ethernet header
        assert_eq!(buffer.len(), 14);
    }
    
    // ============================================================================
    // Socket Tests
    // ============================================================================
    
    #[test]
    fn test_socket_address_creation() {
        let addr = IpAddress::V4([127, 0, 0, 1]);
        // Basic socket address test
        assert!(matches!(addr, IpAddress::V4(_)));
    }
    
    // ============================================================================
    // Loopback Driver Tests
    // ============================================================================
    
    #[test]
    fn test_loopback_send_receive() {
        use drivers::loopback::LoopbackDevice;
        use drivers::NetworkDevice;
        
        let dev = LoopbackDevice::new();
        let data = b"Hello, loopback!";
        
        // Send
        assert!(dev.send(data).is_ok());
        
        // Receive
        let packets = dev.receive().unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].as_slice(), data);
        
        // Stats
        let stats = dev.stats();
        assert_eq!(stats.tx_packets, 1);
        assert_eq!(stats.rx_packets, 1);
    }
    
    // ============================================================================
    // Fragmentation Tests
    // ============================================================================
    
    #[test]
    fn test_fragment_key_creation() {
        use ip::fragmentation::FragmentKey;
        
        let key = FragmentKey {
            src: [192, 168, 1, 1],
            dst: [192, 168, 1, 2],
            id: 12345,
            protocol: 6, // TCP
        };
        
        assert_eq!(key.id, 12345);
        assert_eq!(key.protocol, 6);
    }
}
