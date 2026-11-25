#![no_std]

//! Pure host-testable supervision, dependency DAG, and canonical service table for `init_server`.

pub mod dependency;
pub mod service;
pub mod service_manager;
pub mod service_table;
pub mod supervisor;

pub use service::{Service, SERVICE_STABLE_MS};
