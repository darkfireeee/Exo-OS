#![no_std]

//! Pure host-testable library surface for `exo-network-server`.
//!
//! Exposes DHCP state machine, ICMP echo rewriting, LPM routing table,
//! generational socket table, TCP state store, transient drain policy,
//! buffer pool, statistics, and wire protocol without bare-metal syscalls.

pub mod buf_pool;
pub mod dhcp;
pub mod icmp;
pub mod protocol;
pub mod routing;
pub mod socket_table;
pub mod stats;
pub mod tcp_store;
