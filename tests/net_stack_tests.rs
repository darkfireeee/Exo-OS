// tests/net_stack_tests.rs - Network Stack Tests
// Comprehensive tests pour le stack réseau production

#[cfg(test)]
mod tests {
    use crate::net::*;

    // ========================================================================
    // Buffer Tests
    // ========================================================================

    #[test]
    fn test_netbuffer_basic() {
        let mut buf = buffer::NetBuffer::new(1024);
        assert_eq!(buf.capacity(), 1024);
        assert_eq!(buf.len(), 0);

        let data = b"Hello, World!";
        buf.write(data).unwrap();
        assert_eq!(buf.len(), data.len());

        let read_data = buf.read(data.len()).unwrap();
        assert_eq!(read_data, data);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_netbuffer_zero_copy() {
        let buf = buffer::NetBuffer::new(2048);
        assert!(buf.is_dma_capable());
    }

    // ========================================================================
    // TCP Tests
    // ========================================================================

    #[test]
    fn test_tcp_connection_state() {
        // Test TCP state machine
        use tcp::TcpState;
        
        let mut state = TcpState::Closed;
        assert_eq!(state, TcpState::Closed);
    }

    #[test]
    fn test_tcp_window_scaling() {
        // Test window scaling calculation
        let window = 65535u32;
        let scale = 7u8;
        let scaled = window << scale;
        assert_eq!(scaled, 8388480);
    }

    // ========================================================================
    // UDP Tests
    // ========================================================================

    #[test]
    fn test_udp_checksum() {
        let data = b"test data";
        // UDP checksum calculation test
        // TODO: Implement checksum function and test
    }

    // ========================================================================
    // Socket Tests
    // ========================================================================

    #[test]
    fn test_socket_creation() {
        use socket::*;
        
        // Create TCP socket
        let sockfd = sys_socket(2, 1, 6).unwrap(); // AF_INET, SOCK_STREAM, TCP
        assert!(sockfd > 0);
    }

    #[test]
    fn test_socket_bind() {
        use socket::*;
        
        let sockfd = sys_socket(2, 1, 6).unwrap();
        let addr = SocketAddr::from_ipv4([127, 0, 0, 1], 8080);
        
        // Bind to localhost:8080
        assert!(sys_bind(sockfd, &addr).is_ok());
    }

    // ========================================================================
    // ARP Tests
    // ========================================================================

    #[test]
    fn test_arp_cache() {
        use arp::ARP_CACHE;
        
        let ip = [192, 168, 1, 1];
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        
        ARP_CACHE.insert(ip, mac);
        assert_eq!(ARP_CACHE.lookup(ip), Some(mac));
    }

    #[test]
    fn test_arp_timeout() {
        use arp::ARP_CACHE;
        
        let ip = [192, 168, 1, 2];
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        
        ARP_CACHE.insert(ip, mac);
        
        // Cache should be valid immediately
        assert!(ARP_CACHE.lookup(ip).is_some());
        
        // TODO: Test timeout après 5 minutes
    }

    // ========================================================================
    // DHCP Tests
    // ========================================================================

    #[test]
    fn test_dhcp_client_creation() {
        use dhcp::DhcpClient;
        
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let client = DhcpClient::new(mac);
        
        assert_eq!(client.state(), dhcp::DhcpState::Init);
        assert!(client.assigned_ip().is_none());
    }

    #[test]
    fn test_dhcp_discover_packet() {
        use dhcp::DhcpClient;
        
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let client = DhcpClient::new(mac);
        
        let discover = client.send_discover();
        assert!(discover.len() > 0);
    }

    // ========================================================================
    // DNS Tests
    // ========================================================================

    #[test]
    fn test_dns_client_creation() {
        use dns::DNS_CLIENT;
        
        DNS_CLIENT.add_dns_server([8, 8, 8, 8]);
        DNS_CLIENT.add_dns_server([1, 1, 1, 1]);
    }

    #[test]
    fn test_dns_cache() {
        use dns::DNS_CLIENT;
        
        let hostname = "example.com";
        let ip = [93, 184, 216, 34];
        
        DNS_CLIENT.cache_insert(hostname, ip, 300);
        
        // TODO: Test cache lookup
    }

    // ========================================================================
    // Epoll Tests
    // ========================================================================

    #[test]
    fn test_epoll_create() {
        use socket::epoll::*;
        
        let epfd = sys_epoll_create1(0).unwrap();
        assert!(epfd > 0);
    }

    #[test]
    fn test_epoll_ctl() {
        use socket::epoll::*;
        
        let epfd = sys_epoll_create1(0).unwrap();
        
        // Create a socket
        let sockfd = socket::sys_socket(2, 1, 6).unwrap();
        
        // Add socket to epoll
        let event = EpollEvent {
            events: EPOLLIN,
            data: sockfd as u64,
        };
        
        assert!(sys_epoll_ctl(epfd, 1, sockfd, Some(event)).is_ok());
    }

    // ========================================================================
    // Poll Tests
    // ========================================================================

    #[test]
    fn test_poll_basic() {
        use socket::poll::*;
        
        let sockfd = socket::sys_socket(2, 1, 6).unwrap();
        
        let mut fds = [PollFd {
            fd: sockfd as i32,
            events: POLLIN,
            revents: 0,
        }];
        
        // Non-blocking poll
        let result = sys_poll(&mut fds, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_select_basic() {
        use socket::poll::*;
        
        let mut readfds = FdSet::new();
        fd_set(0, &mut readfds);
        
        assert!(fd_isset(0, &readfds));
        
        fd_clr(0, &mut readfds);
        assert!(!fd_isset(0, &readfds));
    }

    // ========================================================================
    // Performance Tests
    // ========================================================================

    #[test]
    fn test_buffer_throughput() {
        // Test buffer write/read throughput
        let mut buf = buffer::NetBuffer::new(65536);
        let data = vec![0xAAu8; 1500];
        
        let start = crate::time::monotonic_time();
        
        for _ in 0..1000 {
            buf.write(&data).unwrap();
            let _ = buf.read(data.len()).unwrap();
        }
        
        let elapsed = crate::time::monotonic_time() - start;
        let throughput = (1000 * 1500 * 8) as f64 / (elapsed as f64 / 1_000_000.0);
        
        log::info!("Buffer throughput: {:.2} Mbps", throughput / 1_000_000.0);
    }

    #[test]
    fn test_socket_creation_performance() {
        let start = crate::time::monotonic_time();
        
        for _ in 0..1000 {
            let sockfd = socket::sys_socket(2, 1, 6).unwrap();
            socket::sys_close(sockfd).unwrap();
        }
        
        let elapsed = crate::time::monotonic_time() - start;
        let ops_per_sec = 1000.0 / (elapsed as f64 / 1_000_000.0);
        
        log::info!("Socket creation: {:.0} ops/sec", ops_per_sec);
    }

    // ========================================================================
    // Integration Tests
    // ========================================================================

    #[test]
    fn test_tcp_loopback() {
        use socket::*;
        
        // Create server socket
        let server_fd = sys_socket(2, 1, 6).unwrap();
        let addr = SocketAddr::from_ipv4([127, 0, 0, 1], 9999);
        sys_bind(server_fd, &addr).unwrap();
        sys_listen(server_fd, 10).unwrap();
        
        // Create client socket
        let client_fd = sys_socket(2, 1, 6).unwrap();
        
        // TODO: Connect and transfer data
    }

    #[test]
    fn test_udp_send_recv() {
        use socket::*;
        
        let sockfd = sys_socket(2, 2, 17).unwrap(); // AF_INET, SOCK_DGRAM, UDP
        let addr = SocketAddr::from_ipv4([127, 0, 0, 1], 10000);
        sys_bind(sockfd, &addr).unwrap();
        
        // TODO: Send/receive UDP packets
    }
}

// ============================================================================
// Benchmark Tests
// ============================================================================

#[cfg(test)]
mod benchmarks {
    use super::*;

    #[test]
    fn bench_netbuffer_allocation() {
        let iterations = 100000;
        let start = crate::time::monotonic_time();
        
        for _ in 0..iterations {
            let _buf = buffer::NetBuffer::new(2048);
        }
        
        let elapsed = crate::time::monotonic_time() - start;
        let ns_per_op = (elapsed * 1000) / iterations;
        
        log::info!("NetBuffer allocation: {} ns/op", ns_per_op);
    }

    #[test]
    fn bench_arp_cache_lookup() {
        use arp::ARP_CACHE;
        
        // Populate cache
        for i in 0..1000 {
            let ip = [192, 168, (i / 256) as u8, (i % 256) as u8];
            let mac = [0, 0, 0, 0, (i / 256) as u8, (i % 256) as u8];
            ARP_CACHE.insert(ip, mac);
        }
        
        let iterations = 1000000;
        let start = crate::time::monotonic_time();
        
        for i in 0..iterations {
            let ip = [192, 168, ((i % 1000) / 256) as u8, ((i % 1000) % 256) as u8];
            let _ = ARP_CACHE.lookup(ip);
        }
        
        let elapsed = crate::time::monotonic_time() - start;
        let ns_per_op = (elapsed * 1000) / iterations;
        
        log::info!("ARP cache lookup: {} ns/op", ns_per_op);
    }

    #[test]
    fn bench_epoll_operations() {
        use socket::epoll::*;
        
        let epfd = sys_epoll_create1(0).unwrap();
        
        // Add 1000 sockets
        for i in 0..1000 {
            let sockfd = socket::sys_socket(2, 1, 6).unwrap();
            let event = EpollEvent {
                events: EPOLLIN | EPOLLOUT,
                data: sockfd as u64,
            };
            sys_epoll_ctl(epfd, 1, sockfd, Some(event)).unwrap();
        }
        
        // Benchmark epoll_wait
        let iterations = 10000;
        let mut events = vec![EpollEvent { events: 0, data: 0 }; 128];
        let start = crate::time::monotonic_time();
        
        for _ in 0..iterations {
            let _ = sys_epoll_wait(epfd, &mut events, 0);
        }
        
        let elapsed = crate::time::monotonic_time() - start;
        let ns_per_op = (elapsed * 1000) / iterations;
        
        log::info!("epoll_wait: {} ns/op", ns_per_op);
    }
}
