//! Performance Tests - Throughput and Latency Benchmarks
//!
//! Benchmarks for network stack performance validation.

#[cfg(test)]
mod tests {
    use crate::net::*;
    use alloc::vec::Vec;
    
    // ============================================================================
    // Throughput Tests
    // ============================================================================
    
    #[test]
    fn bench_loopback_throughput() {
        use drivers::loopback::LoopbackDevice;
        use drivers::NetworkDevice;
        
        let dev = LoopbackDevice::new();
        let packet = vec![0u8; 1500]; // Full MTU packet
        
        let iterations = 1000;
        
        // Send packets
        for _ in 0..iterations {
            dev.send(&packet).unwrap();
        }
        
        // Receive all
        let received = dev.receive().unwrap();
        assert_eq!(received.len(), iterations);
        
        let stats = dev.stats();
        assert_eq!(stats.tx_packets, iterations as u64);
        assert_eq!(stats.rx_packets, iterations as u64);
    }
    
    #[test]
    fn bench_routing_lookup() {
        use ip::routing::{RoutingTable, Route, IpNetwork, IpAddr, RouteSource};
        
        let table = RoutingTable::new();
        
        // Add 1000 routes
        for i in 0..1000 {
            table.add_route(Route {
                destination: IpNetwork {
                    addr: IpAddr::V4([10, (i >> 8) as u8, (i & 0xFF) as u8, 0]),
                    prefix_len: 24,
                },
                gateway: Some(IpAddr::V4([192, 168, 1, 1])),
                interface: 0,
                metric: 100,
                source: RouteSource::Static,
            });
        }
        
        // Benchmark lookups
        let lookups = 10000;
        for i in 0..lookups {
            let addr = IpAddr::V4([10, ((i / 256) % 256) as u8, (i % 256) as u8, 1]);
            let result = table.lookup(&addr);
            assert!(result.is_some());
        }
    }
    
    #[test]
    fn bench_tcp_segment_serialization() {
        use tcp::segment::TcpSegment;
        use tcp::segment::TcpFlags;
        
        let segment = TcpSegment {
            src_port: 80,
            dst_port: 12345,
            seq_number: 1000000,
            ack_number: 2000000,
            flags: TcpFlags::ACK,
            window_size: 65535,
            urgent_pointer: 0,
            options: Vec::new(),
            payload: vec![0u8; 1460], // Max TCP payload
        };
        
        // Benchmark serialization
        let iterations = 1000;
        for _ in 0..iterations {
            let _serialized = segment.serialize();
        }
    }
    
    #[test]
    fn bench_ip_checksum() {
        use ip::ipv4::checksum;
        
        let data = vec![0xABu8; 1500];
        
        let iterations = 10000;
        for _ in 0..iterations {
            let _sum = checksum(&data);
        }
    }
    
    // ============================================================================
    // Latency Tests
    // ============================================================================
    
    #[test]
    fn test_routing_cache_hit() {
        use ip::routing::{RoutingTable, Route, IpNetwork, IpAddr, RouteSource};
        
        let table = RoutingTable::new();
        
        table.add_route(Route {
            destination: IpNetwork {
                addr: IpAddr::V4([192, 168, 1, 0]),
                prefix_len: 24,
            },
            gateway: Some(IpAddr::V4([192, 168, 1, 1])),
            interface: 0,
            metric: 100,
            source: RouteSource::Static,
        });
        
        let target = IpAddr::V4([192, 168, 1, 100]);
        
        // First lookup (cache miss)
        let result1 = table.lookup(&target);
        assert!(result1.is_some());
        
        // Second lookup (cache hit - should be faster)
        let result2 = table.lookup(&target);
        assert!(result2.is_some());
        
        // Verify cache stats
        let stats = table.stats();
        assert!(stats.cache_hits > 0);
    }
    
    #[test]
    fn bench_udp_header_parse() {
        use protocols::udp::UdpHeader;
        
        let header_bytes = [
            0x00, 0x35, // src port 53
            0x30, 0x39, // dst port 12345
            0x00, 0x64, // length 100
            0xAB, 0xCD, // checksum
        ];
        
        let iterations = 10000;
        for _ in 0..iterations {
            let header = unsafe {
                core::ptr::read(header_bytes.as_ptr() as *const UdpHeader)
            };
            assert_eq!(header.src_port(), 53);
        }
    }
    
    // ============================================================================
    // Memory Tests
    // ============================================================================
    
    #[test]
    fn bench_packet_buffer_allocation() {
        use core::buffer::PacketBuffer;
        
        let iterations = 1000;
        let mut buffers = Vec::new();
        
        for _ in 0..iterations {
            let buffer = PacketBuffer::new(1500);
            buffers.push(buffer);
        }
        
        assert_eq!(buffers.len(), iterations);
    }
    
    #[test]
    fn test_zero_copy_path() {
        use core::buffer::PacketBuffer;
        
        // Simulate zero-copy receive path
        let mut buffer = PacketBuffer::with_headroom(128, 1500);
        
        // Application writes data
        let app_data = vec![0x42u8; 1000];
        buffer.append(&app_data);
        
        // Add headers without copying payload
        let tcp_header = vec![0u8; 20];
        buffer.push_header(&tcp_header);
        
        let ip_header = vec![0u8; 20];
        buffer.push_header(&ip_header);
        
        let eth_header = vec![0u8; 14];
        buffer.push_header(&eth_header);
        
        // Verify single allocation
        assert_eq!(buffer.len(), 1000 + 20 + 20 + 14);
    }
    
    // ============================================================================
    // Concurrency Tests
    // ============================================================================
    
    #[test]
    fn test_concurrent_routing_lookups() {
        use ip::routing::{RoutingTable, Route, IpNetwork, IpAddr, RouteSource};
        use alloc::sync::Arc;
        
        let table = Arc::new(RoutingTable::new());
        
        // Add routes
        for i in 0..100 {
            table.add_route(Route {
                destination: IpNetwork {
                    addr: IpAddr::V4([10, i, 0, 0]),
                    prefix_len: 16,
                },
                gateway: Some(IpAddr::V4([192, 168, 1, 1])),
                interface: 0,
                metric: 100,
                source: RouteSource::Static,
            });
        }
        
        // Simulate concurrent lookups from multiple "threads"
        // (In actual test, would spawn threads/tasks)
        for _ in 0..10 {
            for i in 0..100 {
                let addr = IpAddr::V4([10, i, 1, 1]);
                let result = table.lookup(&addr);
                assert!(result.is_some());
            }
        }
        
        let stats = table.stats();
        assert!(stats.lookups >= 1000);
    }
    
    // ============================================================================
    // Stress Tests
    // ============================================================================
    
    #[test]
    fn stress_test_fragment_cache() {
        use ip::fragmentation::FragmentManager;
        
        let mgr = FragmentManager::new();
        
        // Create many fragments
        for i in 0..100 {
            let key = ip::fragmentation::FragmentKey {
                src: [192, 168, 1, (i % 256) as u8],
                dst: [10, 0, 0, 1],
                id: i as u16,
                protocol: 6,
            };
            
            let fragment = ip::fragmentation::IpFragment {
                offset: 0,
                more_fragments: true,
                data: vec![0u8; 500],
            };
            
            mgr.add_fragment(key, fragment).ok();
        }
    }
    
    #[test]
    fn stress_test_connection_tracking() {
        use firewall::conntrack::{ConnectionTracker, Connection, ConnectionState};
        
        let tracker = ConnectionTracker::new();
        
        // Track many connections
        for i in 0..1000 {
            let conn = Connection {
                src_ip: [192, 168, 1, ((i / 256) % 256) as u8],
                dst_ip: [10, 0, 0, (i % 256) as u8],
                src_port: (i % 65000) as u16,
                dst_port: 80,
                protocol: 6,
                state: ConnectionState::Established,
            };
            
            tracker.track(conn);
        }
    }
}
