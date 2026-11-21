//! # kernel/src/fs/elf_loader_impl/mod.rs
//!
//! Modular implementation of `ElfLoader` to load ELF binaries from ExoFS.
//! Split into specialized submodules:
//! - `trace` : Tracing and port 0xE9 diagnostic primitives.
//! - `registry` : ELF blob registry and memory segment metadata.
//! - `parser` : ELF headers and Program Headers decoding and validation.
//! - `image` : Segment and VMA installation in the address space.
//! - `stack` : Initial SysV user stack construction and argument passing.
//! - `cache` : Demand-paging dedicated ELF blob cache avoiding SLUB pressure.

pub mod cache;
pub mod image;
pub mod parser;
pub mod registry;
pub mod stack;
pub mod trace;

use alloc::boxed::Box;
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

use cache::{load_blob_cached, read_blob_from_cache, resolve_blob_id};
use image::{install_anon_rw_vma, install_elf_image, install_stack_vma};
use parser::{
    compute_et_dyn_span, elf_type, read_interpreter_path, validate_elf_header,
    validate_user_segment_range, ET_DYN, USER_ELF_BASE_MAX,
};
use crate::arch::constants::USER_ELF_BASE_MIN;
use registry::{
    clear_elf_segments, lookup_elf_blob, lookup_elf_segments, register_elf_blob, ElfWalkAllocator,
};
use stack::{
    build_initial_process_stack_with_auxv, generate_random_16, map_anon_pages, map_stack_pages,
    write_anon_bytes, StackAuxvConfig,
};
use trace::{trace_blob, trace_path};

const PIE_ASLR_BASE_PRIMARY: u64 = 0x0000_0100_0000_0000;
const PIE_ASLR_BASE_ALT: u64 = 0x0000_0300_0000_0000;
const INTERP_ASLR_BASE_PRIMARY: u64 = 0x0000_0200_0000_0000;
const INTERP_ASLR_BASE_ALT: u64 = 0x0000_0400_0000_0000;
const ASLR_SLIDE_PAGE_MASK: u64 = 0x00FF_FFFF;

#[inline]
fn intervals_overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Computes `(load_bias, mapped_span)` for an `ET_DYN` image using CSPRNG ASLR,
/// avoiding any already-mapped `(min, max)` interval.
fn choose_et_dyn_load_bias(
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
        // ── 1. Resolve ExoFS path -> BlobId ────────────────────────────
        trace_path(b"elf: load ", path);
        let blob_id = resolve_blob_id(path.as_bytes())?;
        trace_blob(b"elf: resolved ", &blob_id);
        let file_id = register_elf_blob(blob_id).ok_or(ElfLoadError::OutOfMemory)?;
        clear_elf_segments(file_id);

        // ── 2. Read blob from cache ──────────────────────────────────
        let elf_data = read_blob_from_cache(&blob_id)?;

        // ── 3. Validate ELF header and detect PT_INTERP ───────────────────
        validate_elf_header(&elf_data)?;
        let interp_path = read_interpreter_path(&elf_data)?;

        // ── 5. Create new address space ────────────────────────────
        let alloc = ElfWalkAllocator;
        let mut builder = PageTableBuilder::new(&alloc).map_err(|_| ElfLoadError::OutOfMemory)?;
        // Copy kernel entries (PML4[256..512]) so the child can
        // reach the kernel during system calls
        unsafe {
            builder
                .copy_kernel_entries()
                .map_err(|_| ElfLoadError::OutOfMemory)?;
        }
        let pml4_phys = builder.pml4_phys();
        let child_as = Box::new(UserAddressSpace::new(pml4_phys, 0));

        // ── 6. Load executable and optional interpreter (ASLR for ET_DYN) ─
        let rand_bytes = generate_random_16(file_id);
        let rand_u64 = u64::from_le_bytes([
            rand_bytes[0], rand_bytes[1], rand_bytes[2], rand_bytes[3],
            rand_bytes[4], rand_bytes[5], rand_bytes[6], rand_bytes[7],
        ]);
        let rand_interp_u64 = u64::from_le_bytes([
            rand_bytes[8], rand_bytes[9], rand_bytes[10], rand_bytes[11],
            rand_bytes[12], rand_bytes[13], rand_bytes[14], rand_bytes[15],
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

        // ── 7. brk_start = next page above last segment ────────
        let brk_start =
            main_image.brk_end.saturating_add(PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
        child_as.init_heap_bounds(brk_start);

        // ── 7b. Kernel/Userspace shared SignalTcb page (AT_SIGNAL_TCB = 51) ─
        const SIGNAL_TCB_BASE: u64 = 0x0000_7FFF_FF60_0000;
        let signal_tcb_vaddr = SIGNAL_TCB_BASE + ((rand_u64 & 0x7F) * PAGE_SIZE as u64);
        let sig_frame = map_anon_pages(&mut builder, &alloc, signal_tcb_vaddr, PAGE_SIZE)?;
        install_anon_rw_vma(
            &child_as,
            signal_tcb_vaddr,
            PAGE_SIZE,
            crate::memory::virt::VmaFlags::DONTEXPAND,
        )?;
        let _ = sig_frame; // Already zeroed by map_anon_pages

        // ── 7c. Initial static TLS (PT_TLS + GenericTcb) for %fs (MSR_FS_BASE) ─
        // x86_64 SysV Variant II places static TLS blocks below %fs:0 without forcing
        // extra 16-byte padding onto p_memsz when PT_TLS.p_align < 16.
        const INITIAL_TLS_BASE_REGION: u64 = 0x0000_7FFF_FF50_0000;
        let raw_align = main_image.tls_align as usize;
        let tls_align = if raw_align <= 1 { 1 } else { raw_align.next_power_of_two() };
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
        let tls_region_base = (INITIAL_TLS_BASE_REGION + (base_align as u64 - 1))
            & !(base_align as u64 - 1);
        let total_tls_pages = (aligned_block_span / PAGE_SIZE) + 1; // +1 page for GenericTcb at [%fs:0]
        let tls_alloc_len = total_tls_pages * PAGE_SIZE;
        let tls_frames = map_anon_pages(
            &mut builder,
            &alloc,
            tls_region_base,
            tls_alloc_len,
        )?;
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

        // Initialize unified Variant II TCB at tls_base ([%fs:0]) matching GenericTcb & musl struct pthread:
        // +0x00 tcb_ptr (self), +0x08 dtv, +0x10 prev, +0x18 next, +0x20 sysinfo, +0x28 canary,
        // +0x30 tid, +0x34 errno, +0x38 tls_end, +0x40 tls_len, +0x48 tcb_len, +0x50 os_specific,
        // +0x60 initial DTV [mod_cnt, mod1_tls].
        let stack_canary = u64::from_le_bytes([
            rand_bytes[8], rand_bytes[9], rand_bytes[10], rand_bytes[11],
            rand_bytes[12], rand_bytes[13], rand_bytes[14], rand_bytes[15],
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

        // ── 8. User stack: minimal eager bootstrap + 8 MiB demand-paged VMA ─
        const STACK_TOP: u64 = USER_STACK_TOP.as_u64();
        const BOOT_STACK_SIZE: usize = USER_STACK_BOOTSTRAP_SIZE;
        let boot_stack_base = STACK_TOP.saturating_sub(BOOT_STACK_SIZE as u64);
        let stack_vma_size = USER_STACK_DEFAULT_SIZE.saturating_sub(PAGE_SIZE);
        let stack_vma_base = STACK_TOP.saturating_sub(stack_vma_size as u64);

        let stack_frames =
            map_stack_pages(&mut builder, &alloc, boot_stack_base, BOOT_STACK_SIZE)?;
        install_stack_vma(&child_as, stack_vma_base, stack_vma_size)?;
        let interp_base = interp_image.as_ref().map(|img| img.base).unwrap_or(0);
        let auxv_cfg = StackAuxvConfig {
            phdr_vaddr: main_image.phdr_vaddr,
            phnum: main_image.phnum,
            entry_vaddr: main_image.entry_point,
            interp_base,
            vdso_ehdr_vaddr: 0,
            signal_tcb_vaddr,
            seed_hint: file_id,
            execfn: path,
        };
        let executable_stack_top = build_initial_process_stack_with_auxv(
            &stack_frames,
            boot_stack_base,
            STACK_TOP,
            argv,
            envp,
            &auxv_cfg,
        )?;

        // User ELF mappings are kept outside PML4[0]. Re-apply the low
        // supervisor window last so every userspace CR3 can safely run syscall
        // and IRQ paths while the kernel is still linked in low memory.
        builder
            .remap_low_kernel_identity()
            .map_err(|_| ElfLoadError::OutOfMemory)?;

        // ── 9. Finalize ─────────────────────────────────────────────────────
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
        // FIX-25: bytes via cache (no more full reread+rehash per fault).
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

#[cfg(test)]
mod tests {
    use super::parser::*;
    use super::*;

    #[test]
    fn test_validate_load_segment_flags_rejects_write_exec() {
        assert_eq!(
            validate_load_segment_flags(PF_R | PF_W | PF_X),
            Err(ElfLoadError::PermissionDenied)
        );
    }

    #[test]
    fn test_validate_load_segment_flags_allows_standard_segments() {
        assert_eq!(validate_load_segment_flags(PF_R | PF_X), Ok(()));
        assert_eq!(validate_load_segment_flags(PF_R | PF_W), Ok(()));
        assert_eq!(validate_load_segment_flags(PF_R), Ok(()));
    }

    fn make_synthetic_elf(e_type: u16, phdrs: &[(u32, u32, u64, u64, u64)]) -> alloc::vec::Vec<u8> {
        let phoff = 64usize;
        let phnum = phdrs.len();
        let mut buf = alloc::vec![0u8; phoff + phnum * 56];
        buf[0..4].copy_from_slice(b"\x7FELF");
        buf[4] = 2; // ELFCLASS64
        buf[5] = 1; // ELFDATA2LSB
        buf[16..18].copy_from_slice(&e_type.to_le_bytes());
        buf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
        buf[32..40].copy_from_slice(&(phoff as u64).to_le_bytes());
        buf[54..56].copy_from_slice(&56u16.to_le_bytes());
        buf[56..58].copy_from_slice(&(phnum as u16).to_le_bytes());
        for (idx, &(p_type, p_flags, p_offset, p_vaddr, p_memsz)) in phdrs.iter().enumerate() {
            let off = phoff + idx * 56;
            buf[off..off + 4].copy_from_slice(&p_type.to_le_bytes());
            buf[off + 4..off + 8].copy_from_slice(&p_flags.to_le_bytes());
            buf[off + 8..off + 16].copy_from_slice(&p_offset.to_le_bytes());
            buf[off + 16..off + 24].copy_from_slice(&p_vaddr.to_le_bytes());
            buf[off + 32..off + 40].copy_from_slice(&0u64.to_le_bytes());
            buf[off + 40..off + 48].copy_from_slice(&p_memsz.to_le_bytes());
            buf[off + 48..off + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        }
        buf
    }

    #[test]
    fn test_elf_type_and_et_dyn_aslr_non_overlapping() {
        let pie = make_synthetic_elf(ET_DYN, &[(PT_LOAD, PF_R | PF_X, 0, 0, 0x3000)]);
        assert_eq!(validate_elf_header(&pie), Ok(()));
        assert_eq!(elf_type(&pie), Ok(ET_DYN));
        assert_eq!(compute_et_dyn_span(&pie), Ok((0, 0x3000)));

        let et_rel = make_synthetic_elf(1, &[(PT_LOAD, PF_R | PF_X, 0, 0, 0x1000)]);
        assert_eq!(validate_elf_header(&et_rel), Err(ElfLoadError::InvalidElf));

        let (pie_bias, pie_span) = choose_et_dyn_load_bias(
            &pie,
            PIE_ASLR_BASE_PRIMARY,
            PIE_ASLR_BASE_ALT,
            None,
            0x1234,
        )
        .expect("pie aslr");
        assert!(pie_bias >= PIE_ASLR_BASE_PRIMARY);

        // Force interpreter primary base inside pie_span to verify fallback avoids overlap and keeps AT_BASE != 0.
        let (interp_bias, interp_span) = choose_et_dyn_load_bias(
            &pie,
            PIE_ASLR_BASE_PRIMARY,
            INTERP_ASLR_BASE_ALT,
            Some(pie_span),
            0x1234,
        )
        .expect("interp aslr");
        assert_ne!(interp_bias, 0);
        assert!(!intervals_overlap(pie_span, interp_span));
    }

    #[test]
    fn test_auxv_contains_standard_and_exo_entries() {
        use crate::process::auxv::{
            build_auxv, AuxvParams, AT_BASE, AT_CAP_TOKEN, AT_EXECFN, AT_HWCAP2, AT_NULL,
            AT_PLATFORM, AT_SECURE, AT_SIGNAL_TCB,
        };
        let params = AuxvParams {
            phdr_vaddr: 0x100_0000_0040,
            phnum: 4,
            entry_vaddr: 0x100_0000_1000,
            interp_base: INTERP_ASLR_BASE_PRIMARY,
            vdso_ehdr_vaddr: 0,
            signal_tcb_vaddr: 0x7fff_ff60_1000,
            cap_token: 0x7fff_fff0_0000,
            uid: 1000,
            gid: 1000,
            random_ptr: 0x7fff_fff0_0100,
            platform_ptr: 0x7fff_fff0_0200,
            execfn_ptr: 0x7fff_fff0_0210,
            secure: 0,
            hwcap2: 0xdead_beef,
        };
        let entries = build_auxv(&params).expect("auxv");
        let find = |ty: u64| entries.iter().find(|e| e.a_type == ty).map(|e| e.a_val);
        assert_eq!(find(AT_BASE), Some(INTERP_ASLR_BASE_PRIMARY));
        assert_eq!(find(AT_SECURE), Some(0));
        assert_eq!(find(AT_PLATFORM), Some(0x7fff_fff0_0200));
        assert_eq!(find(AT_EXECFN), Some(0x7fff_fff0_0210));
        assert_eq!(find(AT_HWCAP2), Some(0xdead_beef));
        assert_eq!(find(AT_SIGNAL_TCB), Some(0x7fff_ff60_1000));
        assert_eq!(find(AT_CAP_TOKEN), Some(0x7fff_fff0_0000));
        assert_eq!(entries.last().map(|e| e.a_type), Some(AT_NULL));
    }

    #[test]
    fn test_sysv_stack_padding_keeps_rsp_16_byte_aligned() {
        for argc in 0..5usize {
            for envc in 0..5usize {
                for auxc in 1..10usize {
                    let pad = stack::sysv_stack_needs_padding(argc, envc, auxc);
                    let total_slots = argc + envc + (auxc * 2) + 3 + usize::from(pad);
                    assert_eq!(total_slots % 2, 0);
                }
            }
        }
    }
}
