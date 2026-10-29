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
use crate::process::auxv::{build_auxv, AuxvParams};
use crate::process::lifecycle::exec::ElfLoadError;

use super::registry::{
    DynamicLoaderHandoff, InterpreterPath, LoadedElfImage, DYNAMIC_LOADER_PATH_MAX,
};

#[derive(Clone, Copy, Debug, Default)]
pub struct StackAuxvConfig {
    pub phdr_vaddr: u64,
    pub phnum: u64,
    pub entry_vaddr: u64,
    pub interp_base: u64,
    pub vdso_ehdr_vaddr: u64,
    pub signal_tcb_vaddr: u64,
    pub seed_hint: u64,
}

#[inline]
pub fn generate_random_16(seed_hint: u64) -> [u8; 16] {
    #[cfg(target_arch = "x86_64")]
    {
        let tsc = crate::arch::x86_64::cpu::tsc::read_tsc();
        let mut z = tsc.wrapping_add(seed_hint.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        let r0 = z ^ (z >> 27);
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let r1 = z ^ (z >> 31);
        let mut out = [0u8; 16];
        out[0..8].copy_from_slice(&r0.to_le_bytes());
        out[8..16].copy_from_slice(&r1.to_le_bytes());
        out
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let mut out = [0u8; 16];
        out[0..8].copy_from_slice(&seed_hint.to_le_bytes());
        out[8..16].copy_from_slice(&(!seed_hint).to_le_bytes());
        out
    }
}

#[inline]
pub fn build_initial_cap_token(random_16: &[u8; 16], seed_hint: u64) -> [u8; 64] {
    let mut cap = [0u8; 64];
    // CapToken (libs/exo_types/src/cap.rs, 48 bytes padded to 64 bytes on stack):
    // [0..8]   generation: u64 = 1 (active, non-revoked)
    // [8..40]  object_id: [u8; 32]
    // [40..44] rights: u32 = 0x1F (Rights::ALL)
    // [44..46] type_id: u16 = 5 (CapabilityType::ExoFsAccess)
    // [46..48] _pad: [u8; 2] = 0
    cap[0..8].copy_from_slice(&1u64.to_le_bytes());
    cap[8..24].copy_from_slice(random_16);
    cap[24..32].copy_from_slice(&seed_hint.to_le_bytes());
    cap[40..44].copy_from_slice(&0x1Fu32.to_le_bytes());
    cap[44..46].copy_from_slice(&5u16.to_le_bytes());
    cap
}

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

/// Alloue et mappe `n_pages` pages utilisateur anonymes ZEROED (`USER_DATA`) de taille arbitraire.
pub fn map_anon_pages<A: FrameAllocatorForWalk>(
    builder: &mut PageTableBuilder<A>,
    _alloc: &A,
    base_vaddr: u64,
    size_bytes: usize,
) -> Result<Vec<Frame>, ElfLoadError> {
    let n_pages = size_bytes.div_ceil(PAGE_SIZE);
    let mut frames = Vec::new();
    frames
        .try_reserve(n_pages)
        .map_err(|_| ElfLoadError::OutOfMemory)?;
    for page_idx in 0..n_pages {
        let frame =
            buddy::alloc_pages(0, AllocFlags::ZEROED).map_err(|_| ElfLoadError::OutOfMemory)?;
        let virt = VirtAddr::new(base_vaddr.saturating_add((page_idx * PAGE_SIZE) as u64));
        builder
            .map_page(virt, frame, PageFlags::USER_DATA)
            .map_err(|_| ElfLoadError::OutOfMemory)?;
        frames.push(frame);
    }
    Ok(frames)
}

pub fn write_anon_bytes(
    frames: &[Frame],
    base_vaddr: u64,
    user_vaddr: u64,
    bytes: &[u8],
) -> Result<(), ElfLoadError> {
    let region_len = (frames.len() * PAGE_SIZE) as u64;
    let start_off = user_vaddr
        .checked_sub(base_vaddr)
        .ok_or(ElfLoadError::InvalidElf)?;
    let end_off = start_off
        .checked_add(bytes.len() as u64)
        .ok_or(ElfLoadError::InvalidElf)?;
    if end_off > region_len {
        return Err(ElfLoadError::InvalidElf);
    }

    let mut copied = 0usize;
    while copied < bytes.len() {
        let off = start_off as usize + copied;
        let page_idx = off / PAGE_SIZE;
        let page_off = off % PAGE_SIZE;
        let n = (bytes.len() - copied).min(PAGE_SIZE - page_off);
        let dst_virt = phys_to_virt(frames[page_idx].start_address());

        let dst = unsafe {
            core::slice::from_raw_parts_mut((dst_virt.as_u64() as usize + page_off) as *mut u8, n)
        };
        dst.copy_from_slice(&bytes[copied..copied + n]);
        copied += n;
    }
    Ok(())
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
    build_initial_process_stack_with_auxv(
        stack_frames,
        stack_base,
        stack_top,
        argv,
        envp,
        &StackAuxvConfig::default(),
    )
}

pub fn build_initial_process_stack_with_auxv(
    stack_frames: &[Frame; USER_STACK_BOOTSTRAP_PAGES],
    stack_base: u64,
    stack_top: u64,
    argv: &[&str],
    envp: &[&str],
    auxv_cfg: &StackAuxvConfig,
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

    // Push 16-byte AT_RANDOM cookie and 64-byte initial CapToken (AT_CAP_TOKEN = 52)
    let random_16 = generate_random_16(auxv_cfg.seed_hint ^ sp);
    sp = (sp.checked_sub(16).ok_or(ElfLoadError::InvalidElf)?) & !15u64;
    write_stack_bytes(stack_frames, stack_base, sp, &random_16)?;
    let random_ptr = sp;

    let cap_token_bytes = build_initial_cap_token(&random_16, auxv_cfg.seed_hint);
    sp = (sp.checked_sub(64).ok_or(ElfLoadError::InvalidElf)?) & !15u64;
    write_stack_bytes(stack_frames, stack_base, sp, &cap_token_bytes)?;
    let cap_token_ptr = sp;

    let auxv_params = AuxvParams {
        phdr_vaddr: auxv_cfg.phdr_vaddr,
        phnum: auxv_cfg.phnum,
        entry_vaddr: auxv_cfg.entry_vaddr,
        interp_base: auxv_cfg.interp_base,
        vdso_ehdr_vaddr: auxv_cfg.vdso_ehdr_vaddr,
        signal_tcb_vaddr: auxv_cfg.signal_tcb_vaddr,
        cap_token: cap_token_ptr,
        uid: 0,
        gid: 0,
        random_ptr,
    };
    let auxv_entries = build_auxv(&auxv_params).map_err(|_| ElfLoadError::OutOfMemory)?;

    sp &= !15u64;

    // Chaque entrée AuxEntry occupe 2 slots u64 (type, val), donc un nombre pair de u64.
    // Le slot de padding conserve RSP % 16 == 8 à l'entrée de `extern "C" fn _start`
    // sans déplacer argc hors de [RSP] (et les stubs ASM `_start` alignent via `and rsp, -16`).
    let pointer_slots = argv_ptrs.len() + envp_ptrs.len() + (auxv_entries.len() * 2) + 3;
    if pointer_slots & 1 == 0 {
        push_stack_u64(stack_frames, stack_base, &mut sp, 0)?;
    }

    let mut aux_idx = auxv_entries.len();
    while aux_idx != 0 {
        aux_idx -= 1;
        let entry = &auxv_entries[aux_idx];
        push_stack_u64(stack_frames, stack_base, &mut sp, entry.a_val)?;
        push_stack_u64(stack_frames, stack_base, &mut sp, entry.a_type)?;
    }

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
