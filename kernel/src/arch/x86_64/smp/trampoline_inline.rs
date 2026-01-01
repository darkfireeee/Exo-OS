//! AP Trampoline - Binary inclusion approach
//!
//! The trampoline code is assembled separately and included as binary data

/// Pre-assembled trampoline binary (compiled by build.rs)
/// This is a workaround - we include the assembled .bin file directly
pub static TRAMPOLINE_CODE: &[u8] = &[];

/// Trampoline size (will be filled by linker or at runtime)
pub const TRAMPOLINE_MAX_SIZE: usize = 4096; // 4KB max

// For now, let's create a minimal working trampoline in Rust inline assembly
// This avoids linkage issues with external .o files

core::arch::global_asm!(
    r#"
.section .ap_trampoline, "awx"
.code16
.global ap_trampoline_start
.global ap_trampoline_end

ap_trampoline_start:
    cli
    cld
    
    # For now, just halt - we'll implement full trampoline once linkage works
    hlt
    jmp ap_trampoline_start
    
ap_trampoline_end:
    .byte 0
"#
);

// Export symbols for Rust
extern "C" {
    pub static ap_trampoline_start: u8;
    pub static ap_trampoline_end: u8;
}

/// Get trampoline size
pub fn trampoline_size() -> usize {
    unsafe {
        let start = &ap_trampoline_start as *const u8 as usize;
        let end = &ap_trampoline_end as *const u8 as usize;
        end - start
    }
}
