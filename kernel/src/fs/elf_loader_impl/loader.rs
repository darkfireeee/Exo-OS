//! `ExoFsElfLoader` (`ElfLoader` + `FileFaultProvider`) implementation.

use alloc::boxed::Box;

use crate::arch::constants::USER_ELF_BASE_MIN;
use crate::memory::core::layout::{
    USER_STACK_BOOTSTRAP_SIZE, USER_STACK_DEFAULT_SIZE, USER_STACK_TOP,
};
use crate::memory::core::AllocError;
use crate::memory::phys_to_virt;
use crate::memory::virt::fault::demand_paging::FileFaultProvider;
use crate::memory::virt::page_table::builder::PageTableBuilder;
use crate::memory::virt::UserAddressSpace;
use crate::memory::{Frame, PAGE_SIZE};
use crate::process::lifecycle::exec::{ElfLoadError, ElfLoadResult, ElfLoader};

use super::cache::{load_blob_cached, read_blob_from_cache, resolve_blob_id};
use super::image::{install_anon_rw_vma, install_elf_image, install_stack_vma};
use super::parser::{
    compute_et_dyn_span, elf_type, read_interpreter_path, validate_elf_header,
    validate_user_segment_range, ET_DYN, USER_ELF_BASE_MAX,
};
use super::registry::{
    clear_elf_segments, lookup_elf_blob, lookup_elf_segments, register_elf_blob, ElfWalkAllocator,
};
use super::stack::{
    build_initial_process_stack_with_auxv, generate_random_16, map_anon_pages, map_stack_pages,
    write_anon_bytes, StackAuxvConfig,
};
use super::trace::{trace_blob, trace_path};

pub(super) const PIE_ASLR_BASE_PRIMARY: u64 = 0x0000_0100_0000_0000;
pub(super) const PIE_ASLR_BASE_ALT: u64 = 0x0000_0300_0000_0000;
pub(super) const INTERP_ASLR_BASE_PRIMARY: u64 = 0x0000_0200_0000_0000;
pub(super) const INTERP_ASLR_BASE_ALT: u64 = 0x0000_0400_0000_0000;
const ASLR_SLIDE_PAGE_MASK: u64 = 0x00FF_FFFF;

#[inline]
pub(super) fn intervals_overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

pub(super) fn choose_et_dyn_load_bias(
    elf_data: &[u8],
    primary_base: u64,
    alt_base: u64,
    avoid: Option<(u64, u64)>,
    rand_word: u64,
) -> Result<(u64, (u64, u64)), ElfLoadError> {
    let (min_vaddr, max_vaddr) = compute_et_dyn_span(elf_data)?;
    let span = max_vaddr.saturating_sub(min_vaddr);
    let slide = (rand_word & ASLR_SLIDE_PAGE_MASK) * PAGE_SIZE as u64;

    let mut target_base = primary_base.saturating_add(slide);
    if let Some(other) = avoid {
        if intervals_overlap((target_base, target_base.saturating_add(span)), other) {
            target_base = alt_base.saturating_add(slide);
        }
    }
    let mapped_end = target_base
        .checked_add(span)
        .ok_or(ElfLoadError::InvalidElf)?;
    if let Some(other) = avoid {
        if intervals_overlap((target_base, mapped_end), other) {
            return Err(ElfLoadError::InvalidElf);
        }
    }
    validate_user_segment_range(target_base, span as usize)?;
    let load_bias = target_base.wrapping_sub(min_vaddr);
    Ok((load_bias, (target_base, mapped_end)))
}

/// ELF loader implementation via ExoFS.
pub struct ExoFsElfLoader;

unsafe impl Send for ExoFsElfLoader {}
unsafe impl Sync for ExoFsElfLoader {}

impl ElfLoader for ExoFsElfLoader {
    fn load_elf(
        &self,
        path: &str,
        argv: &[&str],
        envp: &[&str],
        _cr3_in: u64,
    ) -> Result<ElfLoadResult, ElfLoadError> {
        trace_path(b"elf: load ", path);
        let blob_id = resolve_blob_id(path.as_bytes())?;
        trace_blob(b"elf: resolved ", &blob_id);
        let file_id = register_elf_blob(blob_id).ok_or(ElfLoadError::OutOfMemory)?;
        clear_elf_segments(file_id);

        let elf_data = read_blob_from_cache(&blob_id)?;
        validate_elf_header(&elf_data)?;
        let interp_path = read_interpreter_path(&elf_data)?;

        let alloc = ElfWalkAllocator;
        let mut builder = PageTableBuilder::new(&alloc).map_err(|_| ElfLoadError::OutOfMemory)?;
        unsafe {
            builder
                .copy_kernel_entries()
                .map_err(|_| ElfLoadError::OutOfMemory)?;
        }
        let pml4_phys = builder.pml4_phys();
        let child_as = Box::new(UserAddressSpace::new(pml4_phys, 0));

        let rand_bytes = generate_random_16(file_id);
        let rand_u64 = u64::from_le_bytes([
            rand_bytes[0],
            rand_bytes[1],
            rand_bytes[2],
            rand_bytes[3],
            rand_bytes[4],
            rand_bytes[5],
            rand_bytes[6],
            rand_bytes[7],
        ]);
        let rand_interp_u64 = u64::from_le_bytes([
            rand_bytes[8],
            rand_bytes[9],
            rand_bytes[10],
            rand_bytes[11],
            rand_bytes[12],
            rand_bytes[13],
            rand_bytes[14],
            rand_bytes[15],
        ]);

        let (main_bias, main_span) = if elf_type(&elf_data)? == ET_DYN {
            choose_et_dyn_load_bias(
                &elf_data,
                PIE_ASLR_BASE_PRIMARY,
                PIE_ASLR_BASE_ALT,
                None,
                rand_u64,
            )?
        } else {
            (0, compute_et_dyn_span(&elf_data)?)
        };
        let main_image = install_elf_image(&elf_data, file_id, &child_as, main_bias)?;
        let mut entry_point = main_image.entry_point;
        let entry_arg0 = 0u64;
        let mut interp_image = None;

        if let Some(_interp) = interp_path {
            let interp_blob = resolve_blob_id(_interp.as_bytes()).map_err(|err| {
                if err == ElfLoadError::NotFound {
                    ElfLoadError::InterpreterNotFound
                } else {
                    err
                }
            })?;
            let interp_file_id = register_elf_blob(interp_blob).ok_or(ElfLoadError::OutOfMemory)?;
            clear_elf_segments(interp_file_id);
            let interp_data = read_blob_from_cache(&interp_blob)?;
            validate_elf_header(&interp_data)?;
            let interp_bias = if elf_type(&interp_data)? == ET_DYN {
                let (bias, _) = choose_et_dyn_load_bias(
                    &interp_data,
                    INTERP_ASLR_BASE_PRIMARY,
                    INTERP_ASLR_BASE_ALT,
                    Some(main_span),
                    rand_interp_u64,
                )?;
                bias
            } else {
                0
            };
            let image = install_elf_image(&interp_data, interp_file_id, &child_as, interp_bias)?;
            entry_point = image.entry_point;
            interp_image = Some(image);
        }
        if entry_point < USER_ELF_BASE_MIN || entry_point >= USER_ELF_BASE_MAX {
            return Err(ElfLoadError::InvalidElf);
        }

        let brk_start =
            main_image.brk_end.saturating_add(PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
        child_as.init_heap_bounds(brk_start);

        const SIGNAL_TCB_BASE: u64 = 0x0000_7FFF_FF60_0000;
        let signal_tcb_vaddr = SIGNAL_TCB_BASE + ((rand_u64 & 0x7F) * PAGE_SIZE as u64);
        let sig_frame = map_anon_pages(&mut builder, &alloc, signal_tcb_vaddr, PAGE_SIZE)?;
        install_anon_rw_vma(
            &child_as,
            signal_tcb_vaddr,
            PAGE_SIZE,
            crate::memory::virt::VmaFlags::DONTEXPAND,
        )?;
        let _ = sig_frame;

        const INITIAL_TLS_BASE_REGION: u64 = 0x0000_7FFF_FF50_0000;
        let raw_align = main_image.tls_align as usize;
        let tls_align = if raw_align <= 1 {
            1
        } else {
            raw_align.next_power_of_two()
        };
        let tls_memsz = main_image.tls_memsz as usize;
        let tls_filesz = (main_image.tls_filesz as usize).min(tls_memsz);
        let tls_block_bytes = if tls_memsz == 0 {
            0
        } else {
            let misalign = (0usize
                .wrapping_sub(tls_memsz)
                .wrapping_sub(main_image.tls_vaddr as usize))
                & (tls_align - 1);
            tls_memsz + misalign
        };
        let base_align = PAGE_SIZE.max(tls_align);
        let aligned_block_span = if tls_block_bytes == 0 {
            0
        } else {
            (tls_block_bytes + base_align - 1) & !(base_align - 1)
        };
        let tls_region_base =
            (INITIAL_TLS_BASE_REGION + (base_align as u64 - 1)) & !(base_align as u64 - 1);
        let total_tls_pages = (aligned_block_span / PAGE_SIZE) + 1;
        let tls_alloc_len = total_tls_pages * PAGE_SIZE;
        let tls_frames = map_anon_pages(&mut builder, &alloc, tls_region_base, tls_alloc_len)?;
        install_anon_rw_vma(
            &child_as,
            tls_region_base,
            tls_alloc_len,
            crate::memory::virt::VmaFlags::from_bits(0),
        )?;

        let tls_base = tls_region_base + aligned_block_span as u64;
        let tdata_vaddr = tls_base - tls_block_bytes as u64;
        if tls_filesz > 0 {
            let src_start = main_image.tls_offset as usize;
            let src_end = src_start.saturating_add(tls_filesz);
            if src_end > elf_data.len() {
                return Err(ElfLoadError::InvalidElf);
            }
            write_anon_bytes(
                &tls_frames,
                tls_region_base,
                tdata_vaddr,
                &elf_data[src_start..src_end],
            )?;
        }

        let stack_canary = u64::from_le_bytes([
            rand_bytes[8],
            rand_bytes[9],
            rand_bytes[10],
            rand_bytes[11],
            rand_bytes[12],
            rand_bytes[13],
            rand_bytes[14],
            rand_bytes[15],
        ]);
        let dtv_vaddr = tls_base + 96;
        let dtv_cnt: u64 = if tls_memsz > 0 { 1 } else { 0 };
        let dtv_mod1: u64 = if tls_memsz > 0 { tdata_vaddr } else { 0 };
        let mut tcb_header = [0u8; 112];
        tcb_header[0..8].copy_from_slice(&tls_base.to_le_bytes());
        tcb_header[8..16].copy_from_slice(&dtv_vaddr.to_le_bytes());
        tcb_header[40..48].copy_from_slice(&stack_canary.to_le_bytes());
        tcb_header[56..64].copy_from_slice(&tls_base.to_le_bytes());
        tcb_header[64..72].copy_from_slice(&(tls_alloc_len as u64).to_le_bytes());
        tcb_header[72..80].copy_from_slice(&(PAGE_SIZE as u64).to_le_bytes());
        tcb_header[80..88].copy_from_slice(&signal_tcb_vaddr.to_le_bytes());
        tcb_header[96..104].copy_from_slice(&dtv_cnt.to_le_bytes());
        tcb_header[104..112].copy_from_slice(&dtv_mod1.to_le_bytes());
        write_anon_bytes(&tls_frames, tls_region_base, tls_base, &tcb_header)?;

        const STACK_TOP: u64 = USER_STACK_TOP.as_u64();
        const BOOT_STACK_SIZE: usize = USER_STACK_BOOTSTRAP_SIZE;
        let boot_stack_base = STACK_TOP.saturating_sub(BOOT_STACK_SIZE as u64);
        let stack_vma_size = USER_STACK_DEFAULT_SIZE.saturating_sub(PAGE_SIZE);
        let stack_vma_base = STACK_TOP.saturating_sub(stack_vma_size as u64);

        let stack_frames =
            map_stack_pages(&mut builder, &alloc, boot_stack_base, BOOT_STACK_SIZE)?;
        install_stack_vma(&child_as, stack_vma_base, stack_vma_size)?;
        let interp_base = interp_image.as_ref().map(|img| img.base).unwrap_or(0);
        let file_mode = crate::syscall::fs_bridge::stored_mode(&blob_id).unwrap_or(0);
        let caller_pid = crate::syscall::fast_path::syscall_current_pid();
        let cred_elevated = if caller_pid != 0 {
            let restr = crate::security::zero_trust::process_restrictions(caller_pid);
            let cred_mismatch = crate::process::core::registry::PROCESS_REGISTRY
                .find_by_pid(crate::process::core::pid::Pid(caller_pid))
                .map(|pcb| {
                    let c = pcb.creds.lock();
                    c.uid != c.euid || c.gid != c.egid
                })
                .unwrap_or(false);
            cred_mismatch
                || (restr & crate::security::zero_trust::restriction_flags::NO_RING1_CRITICAL_IPC)
                    != 0
        } else {
            false
        };
        let at_secure = u64::from(
            (file_mode & 0o6000) != 0 || path.starts_with("/sbin/") || cred_elevated,
        );
        let auxv_cfg = StackAuxvConfig {
            phdr_vaddr: main_image.phdr_vaddr,
            phnum: main_image.phnum,
            entry_vaddr: main_image.entry_point,
            interp_base,
            vdso_ehdr_vaddr: 0,
            signal_tcb_vaddr,
            seed_hint: file_id,
            execfn: path,
            secure: at_secure,
        };
        let executable_stack_top = build_initial_process_stack_with_auxv(
            &stack_frames,
            boot_stack_base,
            STACK_TOP,
            argv,
            envp,
            &auxv_cfg,
        )?;

        builder
            .remap_low_kernel_identity()
            .map_err(|_| ElfLoadError::OutOfMemory)?;

        let child_cr3 = pml4_phys.as_u64();
        let addr_space_ptr = Box::into_raw(child_as) as usize;

        Ok(ElfLoadResult {
            entry_point,
            initial_stack_top: executable_stack_top,
            entry_arg0,
            tls_base,
            tls_size: tls_alloc_len,
            brk_start,
            cr3: child_cr3,
            addr_space_ptr,
            signal_tcb_vaddr,
        })
    }
}

impl FileFaultProvider for ExoFsElfLoader {
    fn load_file_page(
        &self,
        file_id: u64,
        file_offset: u64,
        dest_frame: Frame,
    ) -> Result<(), AllocError> {
        let blob_id = lookup_elf_blob(file_id).ok_or(AllocError::InvalidParams)?;
        let data_arc = load_blob_cached(&blob_id)?;
        let data = &data_arc[..];
        let dst_virt = phys_to_virt(dest_frame.start_address());
        let dst =
            unsafe { core::slice::from_raw_parts_mut(dst_virt.as_u64() as *mut u8, PAGE_SIZE) };

        dst.fill(0);

        let (segments, segment_count) = lookup_elf_segments(file_id);
        if segment_count == 0 {
            let start = file_offset as usize;
            if start >= data.len() {
                return Ok(());
            }
            let end = start.saturating_add(PAGE_SIZE).min(data.len());
            let len = end.saturating_sub(start);
            dst[..len].copy_from_slice(&data[start..end]);
            return Ok(());
        }

        let page_start = file_offset;
        let page_end = page_start.saturating_add(PAGE_SIZE as u64);
        for segment in segments.iter().take(segment_count) {
            if page_end <= segment.file_data_start || page_start >= segment.file_data_end {
                continue;
            }

            let copy_start = page_start.max(segment.file_data_start);
            let copy_end = page_end.min(segment.file_data_end);
            if copy_start >= copy_end || copy_start > usize::MAX as u64 {
                continue;
            }

            let src_start = copy_start as usize;
            if src_start >= data.len() {
                continue;
            }
            let src_end = (copy_end as usize).min(data.len());
            if src_start >= src_end {
                continue;
            }

            let dst_start = copy_start.saturating_sub(page_start) as usize;
            let len = src_end.saturating_sub(src_start);
            if dst_start.saturating_add(len) <= PAGE_SIZE {
                dst[dst_start..dst_start + len].copy_from_slice(&data[src_start..src_end]);
            }
        }
        Ok(())
    }
}

/// Static ELF loader instance.
pub static EXO_ELF_LOADER: ExoFsElfLoader = ExoFsElfLoader;
