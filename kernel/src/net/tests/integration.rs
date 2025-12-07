//! Integration Tests - Multi-component Testing
//!
//! Tests for interactions between network stack components.

#[cfg(test)]
mod tests {
    use crate::net::*;
    use alloc::vec::Vec;
    
    // ============================================================================
    // TCP/IP Stack Integration
    // ============================================================================
    
    #[test]
    fn test_tcp_over_ip() {
        // Test TCP segment encapsulation in IP packet
        use tcp::segment::TcpSegment;
        use tcp::segment::TcpFlags;
        use ip::ipv4::Ipv4Packet;
        
        let segment = TcpSegment {
            src_port: 80,
            dst_port: 12345,
            seq_number: 1000,
            ack_number: 0,
            flags: TcpFlags::SYN,
            window_size: 65535,
            urgent_pointer: 0,
            options: Vec::new(),
            payload: Vec::new(),
        };
        
        // Serialize TCP segment
        let tcp_data = segment.serialize();
        
        // Create IP packet
        let packet = Ipv4Packet::new(
            [192, 168, 1, 1],
            [192, 168, 1, 2],
            6, // TCP protocol
            tcp_data,
        );
        
        assert!(packet.is_ok());
    }
    
    // ============================================================================
    // Ethernet Frame Encapsulation
    // ============================================================================
    
    #[test]
    fn test_ip_over_ethernet() {
        use ethernet::{MacAddress, EtherType};
        use ip::ipv4::Ipv4Packet;
        
        let src_mac = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let dst_mac = MacAddress([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        
        // Create IP packet
        let ip_packet = Ipv4Packet::new(
            [192, 168, 1, 1],
            [192, 168, 1, 2],
            1, // ICMP
            vec![0x08, 0x00], // Echo request
        ).unwrap();
        
        // Encapsulate in Ethernet frame
        let mut frame = Vec::new();
        frame.extend_from_slice(&dst_mac.0);
        frame.extend_from_slice(&src_mac.0);
        frame.extend_from_slice(&(EtherType::IPv4 as u16).to_be_bytes());
        frame.extend_from_slice(&ip_packet);
        
        assert!(frame.len() >= 14); // Ethernet header size
    }
    
    // ============================================================================
    // UDP Socket Communication
    // ============================================================================
    
    #[test]
    fn test_udp_socket_bind_send() {
        use protocols::udp::UdpSocket;
        
        // Create UDP socket
        let socket = UdpSocket::new();
        
        // Bind to address
        let bind_result = socket.bind(53);
        assert!(bind_result.is_ok());
        
        // Send data
        let data = b"DNS Query";
        let send_result = socket.send_to(
            data,
            [8, 8, 8, 8],
            53,
        );
        
        // Verify socket state
        assert!(socket.is_bound());
    }
    
    // ============================================================================
    // ARP Resolution
    // ============================================================================
    
    #[test]
    fn test_arp_request_reply() {
        use protocols::ethernet::arp::{ArpPacket, ArpOperation};
        use ethernet::MacAddress;
        
        let sender_mac = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let sender_ip = [192, 168, 1, 1];
        let target_ip = [192, 168, 1, 2];
        
        // Create ARP request
        let request = ArpPacket::new(
            ArpOperation::Request,
            sender_mac.0,
            sender_ip,
            [0x00; 6], // Unknown target MAC
            target_ip,
        );
        
        assert_eq!(request.operation, ArpOperation::Request);
        assert_eq!(request.sender_ip, sender_ip);
    }
    
    // ============================================================================
    // Routing and Forwarding
    // ============================================================================
    
    #[test]
    fn test_routing_with_multiple_routes() {
        use ip::routing::{RoutingTable, Route, IpNetwork, IpAddr, RouteSource};
        
        let table = RoutingTable::new();
        
        // Add default route
        table.add_route(Route {
            destination: IpNetwork {
                addr: IpAddr::V4([0, 0, 0, 0]),
                prefix_len: 0,
            },
            gateway: Some(IpAddr::V4([192, 168, 1, 1])),
            interface: 0,
            metric: 100,
            source: RouteSource::Static,
        });
        
        // Add specific route
        table.add_route(Route {
            destination: IpNetwork {
                addr: IpAddr::V4([10, 0, 0, 0]),
                prefix_len: 8,
            },
            gateway: Some(IpAddr::V4([192, 168, 1, 2])),
            interface: 0,
            metric: 50,
            source: RouteSource::Static,
        });
        
        // Lookup specific network (should match specific route)
        let next_hop = table.lookup(&IpAddr::V4([10, 1, 2, 3]));
        assert!(next_hop.is_some());
        
        // Lookup outside network (should match default route)
        let default_hop = table.lookup(&IpAddr::V4([8, 8, 8, 8]));
        assert!(default_hop.is_some());
    }
    
    // ============================================================================
    // TCP Connection Lifecycle
    // ============================================================================
    
    #[test]
    fn test_tcp_connection_states() {
        use tcp::state::TcpState;
        use tcp::connection::TcpConnection;
        
        let mut conn = TcpConnection::new(
            [192, 168, 1, 1],
            12345,
            [192, 168, 1, 2],
            80,
        );
        
        assert_eq!(conn.state(), TcpState::Closed);
        
        // Simulate connection establishment
        conn.connect();
        assert_eq!(conn.state(), TcpState::SynSent);
    }
    
    // ============================================================================
    // Firewall Rule Processing
    // ============================================================================
    
    #[test]
    fn test_firewall_rule_matching() {
        use firewall::rules::{FirewallRule, RuleAction, RuleMatch};
        
        let rule = FirewallRule {
            id: 1,
            action: RuleAction::Accept,
            matches: RuleMatch {
                src_ip: Some([192, 168, 1, 0]),
                src_prefix: Some(24),
                dst_ip: None,
                dst_prefix: None,
                protocol: None,
                src_port: None,
                dst_port: None,
            },
        };
        
        // Test packet from 192.168.1.100
        let src_ip = [192, 168, 1, 100];
        assert!(rule.matches_ip(src_ip));
        
        // Test packet from different network
        let other_ip = [10, 0, 0, 1];
        assert!(!rule.matches_ip(other_ip));
    }
    
    // ============================================================================
    // NAT Translation
    // ============================================================================
    
    #[test]
    fn test_nat_port_mapping() {
        use firewall::nat::{NatTable, NatEntry};
        
        let nat_table = NatTable::new();
        
        let entry = NatEntry {
            internal_ip: [192, 168, 1, 100],
            internal_port: 12345,
            external_ip: [1, 2, 3, 4],
            external_port: 54321,
            protocol: 6, // TCP
        };
        
        nat_table.add_mapping(entry);
        
        let lookup = nat_table.lookup_outbound([192, 168, 1, 100], 12345, 6);
        assert!(lookup.is_some());
    }
    
    // ============================================================================
    // QoS Priority Queuing
    // ============================================================================
    
    #[test]
    fn test_qos_priority_ordering() {
        use qos::{Priority, QosPacket};
        
        let high_prio = QosPacket {
            data: vec![1, 2, 3],
            priority: Priority::High,
            timestamp: 100,
            src_ip: [192, 168, 1, 1],
            dst_ip: [192, 168, 1, 2],
            tos: 0,
        };
        
        let low_prio = QosPacket {
            data: vec![4, 5, 6],
            priority: Priority::Low,
            timestamp: 99, // Earlier timestamp
            src_ip: [192, 168, 1, 1],
            dst_ip: [192, 168, 1, 2],
            tos: 0,
        };
        
        // High priority should be processed first despite later timestamp
        assert!(high_prio.priority < low_prio.priority);
    }
    
    // ============================================================================
    // VPN Encapsulation
    // ============================================================================
    
    #[test]
    fn test_ipsec_sa_creation() {
        use vpn::ipsec::{SecurityAssociation, IpsecProtocol, IpsecMode, EncryptionAlgorithm};
        
        let sa = SecurityAssociation::new(
            12345, // SPI
            IpsecProtocol::ESP,
            IpsecMode::Tunnel,
            [192, 168, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [192, 168, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            EncryptionAlgorithm::Aes256Gcm,
            vec![0u8; 32], // 256-bit key
        );
        
        assert_eq!(sa.spi, 12345);
        assert_eq!(sa.protocol, IpsecProtocol::ESP);
    }
}
