#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use exo_loader::dynamic_linker::{abi_export_table, dlstart_stage2, UserJump, AT_BASE, AT_NULL};
use exo_loader::elf::dyn_info::{Elf64Dyn, DT_JMPREL, DT_NULL, DT_PLTRELSZ, DT_RELA, DT_RELASZ};
use exo_loader::elf::reloc_apply::{Elf64Rela, R_X86_64_RELATIVE};
use exo_loader::syscall::{sys_exit_group, sys_log};

#[cfg(all(target_arch = "x86_64", not(test)))]
core::arch::global_asm!(
    ".text",
    ".global _start",
    "_start:",
    "xor rbp, rbp",
    "mov rdi, rsp",
    ".weak _DYNAMIC",
    ".hidden _DYNAMIC",
    "lea rsi, [rip + _DYNAMIC]",
    ".weak __ehdr_start",
    ".hidden __ehdr_start",
    "lea rdx, [rip + __ehdr_start]",
    "and rsp, -16",
    "call _dlstart_c",
    "mov rsp, rdx",
    "xor rdx, rdx",
    "xor rdi, rdi",
    "xor rsi, rsi",
    "jmp rax",
);

#[inline(always)]
unsafe fn self_relocate_table(base: usize, table_vaddr: u64, byte_len: usize) {
    if table_vaddr == 0 || byte_len == 0 {
        return;
    }
    let step = core::mem::size_of::<Elf64Rela>();
    let count = byte_len / step;
    let start = base.wrapping_add(table_vaddr as usize);
    let mut i = 0usize;
    while i < count {
        let rela = unsafe { core::ptr::read_unaligned((start + i * step) as *const Elf64Rela) };
        if (rela.r_info & 0xffff_ffff) as u32 == R_X86_64_RELATIVE {
            let slot = base.wrapping_add(rela.r_offset as usize) as *mut usize;
            let val = (base as isize).wrapping_add(rela.r_addend as isize) as usize;
            unsafe { core::ptr::write_unaligned(slot, val) };
        }
        i += 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn _dlstart_c(
    sp: *const usize,
    dynv: *const Elf64Dyn,
    ehdr_base: usize,
) -> UserJump {
    if sp.is_null() {
        sys_exit_group(22);
    }

    // Stage 1: parse auxv for AT_BASE using zero globals/heap and self-relocate RELATIVE entries.
    let argc = unsafe { *sp };
    let mut cur = unsafe { sp.add(1 + argc + 1) };
    while unsafe { *cur } != 0 {
        cur = unsafe { cur.add(1) };
    }
    let mut auxv = unsafe { cur.add(1) };
    let mut at_base = 0usize;
    loop {
        let a_type = unsafe { *auxv };
        let a_val = unsafe { *auxv.add(1) };
        if a_type == AT_NULL {
            break;
        }
        if a_type == AT_BASE {
            at_base = a_val;
        }
        auxv = unsafe { auxv.add(2) };
    }

    let base = if at_base != 0 { at_base } else { ehdr_base };
    if !dynv.is_null() && base != 0 {
        let mut rela_vaddr = 0u64;
        let mut relasz = 0usize;
        let mut jmprel_vaddr = 0u64;
        let mut pltrelsz = 0usize;
        let mut i = 0usize;
        while i < 512 {
            let d = unsafe { core::ptr::read_unaligned(dynv.add(i)) };
            if d.d_tag == DT_NULL {
                break;
            }
            match d.d_tag {
                DT_RELA => rela_vaddr = d.d_val,
                DT_RELASZ => relasz = d.d_val as usize,
                DT_JMPREL => jmprel_vaddr = d.d_val,
                DT_PLTRELSZ => pltrelsz = d.d_val as usize,
                _ => {}
            }
            i += 1;
        }
        unsafe {
            self_relocate_table(base, rela_vaddr, relasz);
            self_relocate_table(base, jmprel_vaddr, pltrelsz);
        }
    }

    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

    // Prevent GC from discarding ABI exports in --export-dynamic ET_DYN link.
    let exports = abi_export_table();
    unsafe { core::ptr::read_volatile(&exports[0]) };

    match unsafe { dlstart_stage2(sp, base, dynv) } {
        Ok(jump) => jump,
        Err(_) => {
            sys_log(b"[ld-exo] fatal: dynamic link failed\n");
            sys_exit_group(127);
        }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_log(b"[ld-exo] panic\n");
    sys_exit_group(127);
}
