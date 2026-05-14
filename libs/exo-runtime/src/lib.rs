#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimePortKind {
    AsyncRuntime,
    ParallelRuntime,
    Allocator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimePort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: RuntimePortKind,
}

pub const RUNTIME_PORTS: &[RuntimePort] = &[
    RuntimePort {
        name: "async-std",
        vendor_tree: "async-std-upstream",
        kind: RuntimePortKind::AsyncRuntime,
    },
    RuntimePort {
        name: "rayon",
        vendor_tree: "rayon-upstream",
        kind: RuntimePortKind::ParallelRuntime,
    },
    RuntimePort {
        name: "jemallocator",
        vendor_tree: "jemallocator-upstream",
        kind: RuntimePortKind::Allocator,
    },
    RuntimePort {
        name: "snmalloc-rs",
        vendor_tree: "snmalloc-rs-upstream",
        kind: RuntimePortKind::Allocator,
    },
];

pub fn runtime_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f52_5400_u64;
    for i in 0..iterations.max(1) {
        let port = RUNTIME_PORTS[i as usize % RUNTIME_PORTS.len()];
        acc = acc.rotate_left(13) ^ port.name.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}
