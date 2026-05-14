#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServicePortKind {
    BuildTool,
    PackageManager,
    NetworkConfig,
    ServiceManager,
    Bus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServicePort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: ServicePortKind,
}

pub const SERVICE_PORTS: &[ServicePort] = &[
    ServicePort {
        name: "cargo-chef",
        vendor_tree: "cargo-chef-upstream",
        kind: ServicePortKind::BuildTool,
    },
    ServicePort {
        name: "pkgcraft",
        vendor_tree: "pkgcraft-upstream",
        kind: ServicePortKind::PackageManager,
    },
    ServicePort {
        name: "rtnetlink",
        vendor_tree: "rtnetlink-upstream",
        kind: ServicePortKind::NetworkConfig,
    },
    ServicePort {
        name: "systemd",
        vendor_tree: "systemd-upstream",
        kind: ServicePortKind::ServiceManager,
    },
    ServicePort {
        name: "zbus",
        vendor_tree: "zbus-upstream",
        kind: ServicePortKind::Bus,
    },
    ServicePort {
        name: "launchd",
        vendor_tree: "launchd-upstream",
        kind: ServicePortKind::ServiceManager,
    },
];

pub fn services_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f53_5256_u64;
    for i in 0..iterations.max(1) {
        let port = SERVICE_PORTS[i as usize % SERVICE_PORTS.len()];
        acc = acc.rotate_left(15) ^ port.name.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}
