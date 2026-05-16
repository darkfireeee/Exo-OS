#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicsPortKind {
    Windowing,
    Gpu,
    Ui,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphicsPort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: GraphicsPortKind,
    pub phoenix_policy: &'static str,
}

pub const GRAPHICS_PORTS: &[GraphicsPort] = &[
    GraphicsPort {
        name: "winit",
        vendor_tree: "winit-upstream",
        kind: GraphicsPortKind::Windowing,
        phoenix_policy: "recreate-window-and-input-handles-after-switch",
    },
    GraphicsPort {
        name: "wgpu",
        vendor_tree: "wgpu-upstream",
        kind: GraphicsPortKind::Gpu,
        phoenix_policy: "drop-and-recreate-device-resources-after-switch",
    },
    GraphicsPort {
        name: "iced",
        vendor_tree: "iced-upstream",
        kind: GraphicsPortKind::Ui,
        phoenix_policy: "rebuild-widget-state-from-app-model-after-switch",
    },
];

pub fn graphics_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f47_4658_u64;
    for i in 0..iterations.max(1) {
        let port = GRAPHICS_PORTS[i as usize % GRAPHICS_PORTS.len()];
        acc = acc.rotate_left(17) ^ port.vendor_tree.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_stack_declares_phoenix_recovery() {
        for port in GRAPHICS_PORTS {
            assert!(!port.phoenix_policy.is_empty());
        }
    }
}
