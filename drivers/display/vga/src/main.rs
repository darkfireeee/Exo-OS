//! exo-vga-text — Démon d'affichage texte VGA fallback en espace utilisateur
//!
//! Mappe la mémoire vidéo texte 80x25 standard (0xB8000) via SYS_MMIO_MAP
//! et sert de console de secours (fallback) lorsque le GOP / Framebuffer haute
//! résolution n'est pas actif.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use exo_syscall_abi as syscall;
use exo_vga_text::{ColorCode, VgaCell, VgaTextBuffer, VGA_CELLS, VGA_TEXT_PHYS};

pub const VGA_CONSOLE_ENDPOINT: u64 = 21;
const SERVICE_NAME: &[u8] = b"vga_text";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 1. Mapper la région VGA texte 0xB8000 (1 page 4K)
    let mapped = unsafe {
        syscall::syscall2(
            syscall::SYS_MMIO_MAP,
            VGA_TEXT_PHYS as u64,
            4096,
        )
    };

    if mapped > 0 {
        let cells = unsafe {
            core::slice::from_raw_parts_mut(mapped as *mut VgaCell, VGA_CELLS)
        };
        let mut vga = VgaTextBuffer::new(cells);
        vga.set_color(ColorCode::GREEN_ON_BLACK);
        vga.write_all(b"[Exo-OS VGA Text Console Ready]\n");
        vga.set_color(ColorCode::LIGHT_GREY_ON_BLACK);

        // 2. Enregistrer l'endpoint IPC auprès d'ipc_router
        let _ = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_REGISTER,
                SERVICE_NAME.as_ptr() as u64,
                SERVICE_NAME.len() as u64,
                VGA_CONSOLE_ENDPOINT,
            )
        };

        // 3. Boucle d'écoute des requêtes texte de la console
        let mut msg_buf = [0u8; syscall::IPC_ENVELOPE_SIZE];
        loop {
            let rc = unsafe {
                syscall::syscall4(
                    syscall::SYS_IPC_RECV,
                    VGA_CONSOLE_ENDPOINT,
                    msg_buf.as_mut_ptr() as u64,
                    msg_buf.len() as u64,
                    0,
                )
            };

            if rc >= syscall::IPC_HEADER_SIZE as i64 {
                let payload_len = (rc as usize - syscall::IPC_HEADER_SIZE).min(syscall::IPC_INLINE_PAYLOAD_SIZE);
                let payload = &msg_buf[syscall::IPC_HEADER_SIZE..syscall::IPC_HEADER_SIZE + payload_len];

                if payload.len() >= 4 && payload[0..4] == [0xFF, 0xFF, 0, 0] {
                    vga.clear();
                } else {
                    vga.write_all(payload);
                }
            }
        }
    }

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
