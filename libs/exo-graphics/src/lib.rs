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
}

pub const GRAPHICS_PORTS: &[GraphicsPort] = &[
    GraphicsPort {
        name: "winit",
        vendor_tree: "winit-upstream",
        kind: GraphicsPortKind::Windowing,
    },
    GraphicsPort {
        name: "wgpu",
        vendor_tree: "wgpu-upstream",
        kind: GraphicsPortKind::Gpu,
    },
    GraphicsPort {
        name: "iced",
        vendor_tree: "iced-upstream",
        kind: GraphicsPortKind::Ui,
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
