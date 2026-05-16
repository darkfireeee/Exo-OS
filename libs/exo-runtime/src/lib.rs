#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimePortKind {
    AsyncRuntime,
    AsyncSyncIoTypes,
    ParallelRuntime,
    Allocator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeVerdict {
    Native,
    RestrictedPostV02,
    Ring3Only,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimePort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: RuntimePortKind,
    pub verdict: RuntimeVerdict,
    pub phoenix_policy: &'static str,
}

pub const RUNTIME_PORTS: &[RuntimePort] = &[
    RuntimePort {
        name: "tokio",
        vendor_tree: "tokio-upstream",
        kind: RuntimePortKind::AsyncSyncIoTypes,
        verdict: RuntimeVerdict::RestrictedPostV02,
        phoenix_policy: "sync-io-types-only-no-runtime-state",
    },
    RuntimePort {
        name: "rayon",
        vendor_tree: "rayon-upstream",
        kind: RuntimePortKind::ParallelRuntime,
        verdict: RuntimeVerdict::Native,
        phoenix_policy: "worker-pool-recreated-after-switch",
    },
    RuntimePort {
        name: "jemallocator",
        vendor_tree: "jemallocator-upstream",
        kind: RuntimePortKind::Allocator,
        verdict: RuntimeVerdict::Ring3Only,
        phoenix_policy: "ring3-arenas-recreated-after-switch",
    },
    RuntimePort {
        name: "snmalloc-rs",
        vendor_tree: "snmalloc-rs-upstream",
        kind: RuntimePortKind::Allocator,
        verdict: RuntimeVerdict::Native,
        phoenix_policy: "ring3-arenas-recreated-after-switch",
    },
];

pub fn runtime_port_allowed(name: &str) -> bool {
    RUNTIME_PORTS
        .iter()
        .find(|port| port.name == name)
        .map(|port| port.verdict != RuntimeVerdict::Rejected)
        .unwrap_or(false)
}

pub fn runtime_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f52_5400_u64;
    for i in 0..iterations.max(1) {
        let port = RUNTIME_PORTS[i as usize % RUNTIME_PORTS.len()];
        acc = acc.rotate_left(13) ^ port.name.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_std_is_removed_and_tokio_is_restricted() {
        assert!(!runtime_port_allowed("async-std"));
        assert!(RUNTIME_PORTS.iter().all(|port| port.name != "async-std"));
        let tokio = RUNTIME_PORTS
            .iter()
            .find(|port| port.name == "tokio")
            .unwrap();
        assert_eq!(tokio.verdict, RuntimeVerdict::RestrictedPostV02);
    }
}
