//! High-Performance Network Subsystem
//!
//! Production-grade TCP/IP stack optimized for AI workloads.
//! 
//! Performance targets:
//! - 100Gbps+ throughput
//! - <10μs latency
//! - 10M+ concurrent connections
//! - Zero-copy I/O paths
//!
//! Architecture:
//! - Lock-free packet processing
//! - Per-CPU queues (RSS/RPS)
//! - Hardware offload (TSO/GSO/GRO)
//! - Native io_uring integration
//! - RDMA for AI workloads

use alloc::vec::Vec;
use crate::memory::MemoryError;

/// Network stack core
pub mod stack;

/// BSD Socket API (POSIX compatible)
pub mod socket;

/// TCP protocol (production grade)
pub mod tcp;

/// IP layer (IPv4/IPv6)
pub mod ip;

/// Ethernet layer
pub mod ethernet;

/// Network core (sockets, devices, buffers)
pub mod core;

/// Network device drivers
pub mod drivers;

/// Protocol implementations (clean modular architecture)
pub mod protocols;

/// WireGuard VPN
pub mod wireguard;

/// VPN subsystem (IPsec, OpenVPN)
pub mod vpn;

/// Time management for network stack
pub mod time;

/// Firewall (moved from netfilter for better naming)
pub mod firewall;

/// Network services (DHCP, DNS, NTP)
pub mod services;

/// QoS (Quality of Service)
pub mod qos;

/// Load Balancer
pub mod loadbalancer;

/// RDMA (Remote Direct Memory Access)
pub mod rdma;

/// Network Performance Monitoring
pub mod monitoring;

/// Network tests (unit, integration, performance)
#[cfg(test)]
pub mod tests;

/// Network errors
#[derive(Debug)]
pub enum NetError {
    InvalidAddress,
    ConnectionRefused,
    Timeout,
    NotConnected,
    AlreadyConnected,
    BufferFull,
    InvalidPacket,
    NotSupported,
    Memory(MemoryError),
}

impl From<MemoryError> for NetError {
    fn from(e: MemoryError) -> Self {
        NetError::Memory(e)
    }
}

pub type NetResult<T> = Result<T, NetError>;

/// Network address (IPv4 or IPv6)
#[derive(Debug, Clone, Copy)]
pub enum IpAddress {
    V4([u8; 4]),
    V6([u8; 16]),
}

/// Initialize network subsystem
pub fn init() -> NetResult<()> {
    log::info!("Initializing network subsystem (hybrid architecture)");
    core::init()?;
    Ok(())
}
