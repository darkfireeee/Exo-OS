//! # NTP Service Module
//! 
//! Network Time Protocol client for time synchronization

pub mod client;

pub use client::{NtpClient, NtpServer, NtpPacket, NtpTimestamp, NtpError};
pub use client::{init, add_server, sync};
