//! stack.rs — Construction de la pile initiale utilisateur et passage des arguments

use alloc::vec::Vec;
use crate::memory::core::layout::{
    USER_STACK_BOOTSTRAP_PAGES, USER_STACK_BOOTSTRAP_SIZE,
};
use crate::memory::core::PageFlags;
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::page_table::builder::PageTableBuilder;
use crate::memory::virt::page_table::walker::FrameAllocatorForWalk;
use crate::memory::{phys_to_virt, AllocFlags, Frame, PhysAddr, VirtAddr, PAGE_SIZE};
use crate::process::lifecycle::exec::ElfLoadError;

use super::registry::{
    DynamicLoaderHandoff, InterpreterPath, LoadedElfImage, DYNAMIC_LOADER_PATH_MAX,
};

pub const DYNAMIC_LOADER_HANDOFF_MAGIC: u64 = 0x5845_4f4c_4459_4e01; // "XEOLDYN\1"
pub const DYNAMIC_LOADER_HANDOFF_VERSION: u32 = 1;
pub const DYNAMIC_LOADER_HANDOFF_STACK_GAP: u64 = 16 * 1024;

pub fn build_dynamic_handoff(
    executable_path: &str,
    interpreter_path: &InterpreterPath,
    executable: &LoadedElfImage,
    interpreter: &LoadedElfImage,
    executable_stack_top: u64,
) -> DynamicLoaderHandoff {
    let mut handoff = DynamicLoaderHandoff {
        magic: DYNAMIC_LOADER_HANDOFF_MAGIC,
        version: DYNAMIC_LOADER_HANDOFF_VERSION,
        flags: 0,
        executable_base: executable.base,
        executable_entry: executable.entry_point,
        executable_phdr: executable.phdr_vaddr,
        executable_phnum: executable.phnum,
        executable_phent: executable.phent,
        executable_dynamic: executable.dynamic_vaddr,
        executable_dynamic_count: executable.dynamic_count,
        interpreter_base: interpreter.base,
        interpreter_entry: interpreter.entry_point,
        page_size: PAGE_SIZE as u64,
        executable_stack_top,
        executable_path_len: 0,
        interpreter_path_len: interpreter_path.len as u32,
        executable_path: [0; DYNAMIC_LOADER_PATH_MAX],
        interpreter_path: [0; DYNAMIC_LOADER_PATH_MAX],
    };

    let exe = executable_path.as_bytes();
    let exe_len = exe.len().min(DYNAMIC_LOADER_PATH_MAX);
    handoff.executable_path[..exe_len].copy_from_slice(&exe[..exe_len]);
    handoff.executable_path_len = exe_len as u32;
    handoff.interpreter_path[..interpreter_path.len].copy_from_slice(interpreter_path.as_bytes());
    handoff
}

/// Alloue et mappe les pages de pile utilisateur (toutes ZEROED, USER_DATA).
pub fn map_stack_pages<A: FrameAllocatorForWalk>(
    builder: &mut PageTableBuilder<A>,
    _alloc: &A,
    stack_base: u64,
    stack_size: usize,
) -> Result<[Frame; USER_STACK_BOOTSTRAP_PAGES], ElfLoadError> {
    let n_pages = stack_size / PAGE_SIZE;
    if n_pages != USER_STACK_BOOTSTRAP_PAGES {
        return Err(ElfLoadError::InvalidElf);
    }
    let mut frames = [Frame::containing(PhysAddr::new(0)); USER_STACK_BOOTSTRAP_PAGES];
    for page_idx in 0..n_pages {
        let frame =
            buddy::alloc_pages(0, AllocFlags::ZEROED).map_err(|_| ElfLoadError::OutOfMemory)?;
        let virt = VirtAddr::new(stack_base.saturating_add((page_idx * PAGE_SIZE) as u64));
        builder
            .map_page(virt, frame, PageFlags::USER_DATA)
            .map_err(|_| ElfLoadError::OutOfMemory)?;
        frames[page_idx] = frame;
    }
    Ok(frames)
}

pub fn write_stack_bytes(
    stack_frames: &[Frame; USER_STACK_BOOTSTRAP_PAGES],
    stack_base: u64,
    user_vaddr: u64,
    bytes: &[u8],
) -> Result<(), ElfLoadError> {
    let stack_len = USER_STACK_BOOTSTRAP_SIZE as u64;
    let start_off = user_vaddr
        .checked_sub(stack_base)
        .ok_or(ElfLoadError::InvalidElf)?;
    let end_off = start_off
        .checked_add(bytes.len() as u64)
        .ok_or(ElfLoadError::InvalidElf)?;
    if end_off > stack_len {
        return Err(ElfLoadError::InvalidElf);
    }

    let mut copied = 0usize;
    while copied < bytes.len() {
        let off = start_off as usize + copied;
        let page_idx = off / PAGE_SIZE;
        let page_off = off % PAGE_SIZE;
        let n = (bytes.len() - copied).min(PAGE_SIZE - page_off);
        let dst_virt = phys_to_virt(stack_frames[page_idx].start_address());

        let dst = unsafe {
            core::slice::from_raw_parts_mut((dst_virt.as_u64() as usize + page_off) as *mut u8, n)
        };
        dst.copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
    }
    Ok(())
}

pub fn write_stack_u64(
    stack_frames: &[Frame; USER_STACK_BOOTSTRAP_PAGES],
    stack_base: u64,
    user_vaddr: u64,
    value: u64,
) -> Result<(), ElfLoadError> {
    write_stack_bytes(stack_frames, stack_base, user_vaddr, &value.to_ne_bytes())
}

pub fn push_stack_bytes(
    stack_frames: &[Frame; USER_STACK_BOOTSTRAP_PAGES],
    stack_base: u64,
    sp: &mut u64,
    bytes: &[u8],
) -> Result<u64, ElfLoadError> {
    let len = bytes.len().checked_add(1).ok_or(ElfLoadError::InvalidElf)? as u64;
    *sp = sp.checked_sub(len).ok_or(ElfLoadError::InvalidElf)?;
    write_stack_bytes(stack_frames, stack_base, *sp, bytes)?;
    write_stack_bytes(stack_frames, stack_base, *sp + bytes.len() as u64, &[0])?;
    Ok(*sp)
}

pub fn push_stack_u64(
    stack_frames: &[Frame; USER_STACK_BOOTSTRAP_PAGES],
    stack_base: u64,
    sp: &mut u64,
    value: u64,
) -> Result<(), ElfLoadError> {
    *sp = sp.checked_sub(8).ok_or(ElfLoadError::InvalidElf)?;
    write_stack_u64(stack_frames, stack_base, *sp, value)
}

pub fn build_initial_process_stack(
    stack_frames: &[Frame; USER_STACK_BOOTSTRAP_PAGES],
    stack_base: u64,
    stack_top: u64,
    argv: &[&str],
    envp: &[&str],
) -> Result<u64, ElfLoadError> {
    let mut sp = stack_top;
    let mut argv_ptrs: Vec<u64> = Vec::new();
    let mut envp_ptrs: Vec<u64> = Vec::new();
    argv_ptrs
        .try_reserve(argv.len())
        .map_err(|_| ElfLoadError::OutOfMemory)?;
    envp_ptrs
        .try_reserve(envp.len())
        .map_err(|_| ElfLoadError::OutOfMemory)?;

    for arg in argv {
        argv_ptrs.push(push_stack_bytes(
            stack_frames,
            stack_base,
            &mut sp,
            arg.as_bytes(),
        )?);
    }
    for env in envp {
        envp_ptrs.push(push_stack_bytes(
            stack_frames,
            stack_base,
            &mut sp,
            env.as_bytes(),
        )?);
    }

    sp &= !15u64;

    // AT_NULL, puis envp/argv, puis argc. Les binaires no_std Exo-OS lisent
    // argc/argv/envp directement depuis ce contrat de pile. Le slot de padding
    // conserve RSP % 16 == 8 sans déplacer argc hors de [RSP].
    let pointer_slots = argv_ptrs.len() + envp_ptrs.len() + 5;
    if pointer_slots & 1 == 0 {
        push_stack_u64(stack_frames, stack_base, &mut sp, 0)?;
    }
    push_stack_u64(stack_frames, stack_base, &mut sp, 0)?;
    push_stack_u64(stack_frames, stack_base, &mut sp, 0)?;

    push_stack_u64(stack_frames, stack_base, &mut sp, 0)?;
    let mut idx = envp_ptrs.len();
    while idx != 0 {
        idx -= 1;
        push_stack_u64(stack_frames, stack_base, &mut sp, envp_ptrs[idx])?;
    }

    push_stack_u64(stack_frames, stack_base, &mut sp, 0)?;
    idx = argv_ptrs.len();
    while idx != 0 {
        idx -= 1;
        push_stack_u64(stack_frames, stack_base, &mut sp, argv_ptrs[idx])?;
    }
    push_stack_u64(stack_frames, stack_base, &mut sp, argv.len() as u64)?;
    Ok(sp)
}
