//! # Network Monitoring - NetFlow/sFlow Export
//! 
//! Export de métriques réseau pour monitoring externe :
//! - NetFlow v5/v9/IPFIX
//! - sFlow v5
//! - Prometheus metrics
//! - SNMP traps

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::string::String;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// NetFlow record (v5)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NetFlowV5Record {
    pub src_addr: u32,
    pub dst_addr: u32,
    pub next_hop: u32,
    pub input: u16,
    pub output: u16,
    pub packets: u32,
    pub bytes: u32,
    pub first: u32,      // SysUptime at start
    pub last: u32,       // SysUptime at end
    pub src_port: u16,
    pub dst_port: u16,
    pub pad1: u8,
    pub tcp_flags: u8,
    pub protocol: u8,
    pub tos: u8,
    pub src_as: u16,
    pub dst_as: u16,
    pub src_mask: u8,
    pub dst_mask: u8,
    pub pad2: u16,
}

impl NetFlowV5Record {
    pub fn new(
        src_addr: [u8; 4],
        dst_addr: [u8; 4],
        src_port: u16,
        dst_port: u16,
        protocol: u8,
    ) -> Self {
        Self {
            src_addr: u32::from_be_bytes(src_addr),
            dst_addr: u32::from_be_bytes(dst_addr),
            next_hop: 0,
            input: 0,
            output: 0,
            packets: 0,
            bytes: 0,
            first: 0,
            last: 0,
            src_port,
            dst_port,
            pad1: 0,
            tcp_flags: 0,
            protocol,
            tos: 0,
            src_as: 0,
            dst_as: 0,
            src_mask: 0,
            dst_mask: 0,
            pad2: 0,
        }
    }
}

/// NetFlow v5 header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NetFlowV5Header {
    pub version: u16,        // 5
    pub count: u16,          // Number of records
    pub sys_uptime: u32,
    pub unix_secs: u32,
    pub unix_nsecs: u32,
    pub flow_sequence: u32,
    pub engine_type: u8,
    pub engine_id: u8,
    pub sampling_interval: u16,
}

/// NetFlow exporter
pub struct NetFlowExporter {
    collector_addr: String,
    collector_port: u16,
    
    // Flow cache
    flows: SpinLock<BTreeMap<FlowKey, FlowRecord>>,
    
    // Statistics
    sequence: AtomicU64,
    stats: ExporterStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FlowKey {
    pub src_addr: [u8; 4],
    pub dst_addr: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
}

#[derive(Debug, Clone)]
pub struct FlowRecord {
    pub key: FlowKey,
    pub packets: u64,
    pub bytes: u64,
    pub first_seen: u64,
    pub last_seen: u64,
    pub tcp_flags: u8,
}

#[derive(Debug, Clone, Default)]
pub struct ExporterStats {
    pub flows_exported: AtomicU64,
    pub packets_sampled: AtomicU64,
    pub export_errors: AtomicU64,
}

impl NetFlowExporter {
    pub fn new(collector_addr: String, collector_port: u16) -> Self {
        Self {
            collector_addr,
            collector_port,
            flows: SpinLock::new(BTreeMap::new()),
            sequence: AtomicU64::new(0),
            stats: ExporterStats::default(),
        }
    }
    
    /// Record packet
    pub fn record_packet(
        &self,
        src_addr: [u8; 4],
        dst_addr: [u8; 4],
        src_port: u16,
        dst_port: u16,
        protocol: u8,
        size: usize,
        tcp_flags: u8,
    ) {
        let key = FlowKey {
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            protocol,
        };
        
        let now = current_time_ms();
        let mut flows = self.flows.lock();
        
        if let Some(flow) = flows.get_mut(&key) {
            // Update existing flow
            flow.packets += 1;
            flow.bytes += size as u64;
            flow.last_seen = now;
            flow.tcp_flags |= tcp_flags;
        } else {
            // Create new flow
            let flow = FlowRecord {
                key,
                packets: 1,
                bytes: size as u64,
                first_seen: now,
                last_seen: now,
                tcp_flags,
            };
            flows.insert(key, flow);
        }
        
        self.stats.packets_sampled.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Export flows (called periodically)
    pub fn export_flows(&self) -> Result<usize, ExportError> {
        let mut flows = self.flows.lock();
        
        if flows.is_empty() {
            return Ok(0);
        }
        
        // Build NetFlow v5 packet
        let mut records = Vec::new();
        let now = current_time_ms();
        
        for (key, flow) in flows.iter() {
            let mut record = NetFlowV5Record::new(
                key.src_addr,
                key.dst_addr,
                key.src_port,
                key.dst_port,
                key.protocol,
            );
            
            record.packets = flow.packets as u32;
            record.bytes = flow.bytes as u32;
            record.first = (flow.first_seen / 1000) as u32;
            record.last = (flow.last_seen / 1000) as u32;
            record.tcp_flags = flow.tcp_flags;
            
            records.push(record);
            
            // Max 30 records per packet
            if records.len() >= 30 {
                break;
            }
        }
        
        // Create header
        let header = NetFlowV5Header {
            version: 5u16.to_be(),
            count: (records.len() as u16).to_be(),
            sys_uptime: (now as u32).to_be(),
            unix_secs: ((now / 1000) as u32).to_be(),
            unix_nsecs: (((now % 1000) * 1_000_000) as u32).to_be(),
            flow_sequence: (self.sequence.fetch_add(1, Ordering::Relaxed) as u32).to_be(),
            engine_type: 0,
            engine_id: 0,
            sampling_interval: 0,
        };
        
        // Serialize and send
        let mut packet = Vec::new();
        packet.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<NetFlowV5Header>()
            )
        });
        
        for record in &records {
            packet.extend_from_slice(unsafe {
                core::slice::from_raw_parts(
                    record as *const _ as *const u8,
                    core::mem::size_of::<NetFlowV5Record>()
                )
            });
        }
        
        // Send UDP packet to collector
        send_udp(&self.collector_addr, self.collector_port, &packet)?;
        
        // Clear exported flows
        let count = records.len();
        for record in records {
            let key = FlowKey {
                src_addr: record.src_addr.to_be_bytes(),
                dst_addr: record.dst_addr.to_be_bytes(),
                src_port: record.src_port,
                dst_port: record.dst_port,
                protocol: record.protocol,
            };
            flows.remove(&key);
        }
        
        self.stats.flows_exported.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
    
    /// Get statistics
    pub fn statistics(&self) -> (u64, u64, u64) {
        (
            self.stats.flows_exported.load(Ordering::Relaxed),
            self.stats.packets_sampled.load(Ordering::Relaxed),
            self.stats.export_errors.load(Ordering::Relaxed),
        )
    }
}

/// sFlow v5 sample
#[derive(Debug, Clone)]
pub struct SFlowSample {
    pub sequence: u32,
    pub source_id: u32,
    pub sampling_rate: u32,
    pub sample_pool: u32,
    pub drops: u32,
    pub input: u32,
    pub output: u32,
    pub protocol: u32,
    pub src_addr: [u8; 4],
    pub dst_addr: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub tcp_flags: u8,
}

/// sFlow exporter
pub struct SFlowExporter {
    collector_addr: String,
    collector_port: u16,
    sampling_rate: u32,
    
    // State
    sequence: AtomicU64,
    sample_pool: AtomicU64,
}

impl SFlowExporter {
    pub fn new(collector_addr: String, collector_port: u16, sampling_rate: u32) -> Self {
        Self {
            collector_addr,
            collector_port,
            sampling_rate,
            sequence: AtomicU64::new(0),
            sample_pool: AtomicU64::new(0),
        }
    }
    
    /// Sample packet (1 in N sampling)
    pub fn sample_packet(
        &self,
        src_addr: [u8; 4],
        dst_addr: [u8; 4],
        src_port: u16,
        dst_port: u16,
        protocol: u8,
        tcp_flags: u8,
    ) -> Option<SFlowSample> {
        let pool = self.sample_pool.fetch_add(1, Ordering::Relaxed);
        
        if pool % (self.sampling_rate as u64) != 0 {
            return None;
        }
        
        Some(SFlowSample {
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) as u32,
            source_id: 0,
            sampling_rate: self.sampling_rate,
            sample_pool: pool as u32,
            drops: 0,
            input: 0,
            output: 0,
            protocol: protocol as u32,
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            tcp_flags,
        })
    }
}

/// Export errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportError {
    NetworkError,
    SerializationError,
}

// Helper functions (mock)
fn current_time_ms() -> u64 {
    0
}

fn send_udp(addr: &str, port: u16, data: &[u8]) -> Result<(), ExportError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_netflow_exporter() {
        let exporter = NetFlowExporter::new("10.0.0.1".into(), 2055);
        
        // Record packets
        exporter.record_packet([192, 168, 1, 10], [10, 0, 0, 1], 12345, 80, 6, 1500, 0x02);
        exporter.record_packet([192, 168, 1, 10], [10, 0, 0, 1], 12345, 80, 6, 1500, 0x10);
        
        let (flows, packets, _) = exporter.statistics();
        assert_eq!(packets, 2);
    }
}
