#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServicePortKind {
    BuildTool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceVerdict {
    NativeTooling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServicePort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: ServicePortKind,
    pub verdict: ServiceVerdict,
    pub replacement: &'static str,
}

pub const SERVICE_PORTS: &[ServicePort] = &[ServicePort {
    name: "cargo-chef",
    vendor_tree: "cargo-chef-upstream",
    kind: ServicePortKind::BuildTool,
    verdict: ServiceVerdict::NativeTooling,
    replacement: "build-only-tooling",
}];

pub fn service_port_allowed(name: &str) -> bool {
    SERVICE_PORTS
        .iter()
        .find(|port| port.name == name)
        .map(|port| port.verdict == ServiceVerdict::NativeTooling)
        .unwrap_or(false)
}

pub fn services_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f53_5256_u64;
    for i in 0..iterations.max(1) {
        let port = SERVICE_PORTS[i as usize % SERVICE_PORTS.len()];
        acc = acc.rotate_left(15) ^ port.name.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_linux_service_models_are_removed() {
        for name in ["pkgcraft", "rtnetlink", "systemd", "zbus", "launchd"] {
            assert!(!service_port_allowed(name), "{name}");
            assert!(SERVICE_PORTS.iter().all(|port| port.name != name), "{name}");
        }
    }
}
