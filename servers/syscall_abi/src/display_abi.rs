//! # display_abi.rs — ABI du protocole d'affichage et compositeur Exo-OS
//!
//! Spécification: docs/Vision v0.2.0/SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md
//! Structures wire #[repr(C)] pour le partage de surfaces graphiques (SHM)
//! et la synchronisation des zones sales (damage rectangles) avec `fb_server`.

/// Formats de pixels supportés par le Framebuffer
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Unknown = 0,
    Rgbx8888 = 1,
    Bgrx8888 = 2,
    Rgb888 = 3,
    Bgr888 = 4,
    Rgb565 = 5,
}

/// Rectangle de région sale (zone modifiée à rafraîchir à l'écran)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

const _: () = assert!(core::mem::size_of::<DamageRect>() == 16);

/// Grant de surface partagée entre un client graphique et `fb_server` (32 octets)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SurfaceGrant {
    /// Identifiant unique de la surface
    pub surface_id: u32,
    /// Largeur de la surface en pixels
    pub width: u32,
    /// Hauteur de la surface en pixels
    pub height: u32,
    /// Pas (stride) en octets par ligne de balayage
    pub stride_bytes: u32,
    /// Format de pixels (PixelFormat)
    pub format: u32,
    /// Drapeaux de la surface
    pub flags: u32,
    /// Adresse virtuelle mappée dans l'espace client
    pub virt_addr: u64,
}

const _: () = assert!(core::mem::size_of::<SurfaceGrant>() == 32);

// Opcodes du protocole d'affichage IPC `fb_server`
pub const DISPLAY_OP_GET_INFO: u32 = 0x5001;
pub const DISPLAY_OP_CREATE_SURFACE: u32 = 0x5002;
pub const DISPLAY_OP_DESTROY_SURFACE: u32 = 0x5003;
pub const DISPLAY_OP_COMMIT_DAMAGE: u32 = 0x5004;
pub const DISPLAY_OP_FLIP: u32 = 0x5005;
pub const DISPLAY_OP_MOVE_WINDOW: u32 = 0x5006;
pub const DISPLAY_OP_WAYLAND_WIRE: u32 = 0x5010;

// Opcodes IPC VirtIO-GPU (`virtio_drivers` endpoint 13)
pub const VIRTIO_GPU_MSG_TRANSFER_FLUSH: u32 = 2;
pub const VIRTIO_GPU_MSG_MOVE_CURSOR: u32 = 3;
