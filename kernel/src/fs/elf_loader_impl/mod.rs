//! # kernel/src/fs/elf_loader_impl/mod.rs
//!
//! Implémentation modulaire de `ElfLoader` pour charger les binaires ELF depuis ExoFS.
//! Découpé en sous-modules spécialisés :
//! - `trace` : Primitives de traçage et diagnostic port 0xE9.
//! - `registry` : Registre des blobs ELF et métadonnées des segments mémoires.
//! - `parser` : Décodage et validation des en-têtes ELF et Program Headers.
//! - `image` : Installation des segments et VMAs dans l'espace d'adressage.
//! - `stack` : Construction de la pile initiale utilisateur SysV et passage d'arguments.
//! - `cache` : Cache de blobs ELF dédié au demand-paging évitant la pression SLUB.

pub mod cache;
pub mod image;
pub mod parser;
pub mod registry;
pub mod stack;
pub mod trace;

use alloc::boxed::Box;
use crate::memory::core::layout::{
    USER_STACK_BOOTSTRAP_SIZE, USER_STACK_TOP,
};
use crate::memory::core::AllocError;
use crate::memory::phys_to_virt;
use crate::memory::virt::fault::demand_paging::FileFaultProvider;
use crate::memory::virt::page_table::builder::PageTableBuilder;
use crate::memory::virt::UserAddressSpace;
use crate::memory::{Frame, PAGE_SIZE};
use crate::process::lifecycle::exec::{ElfLoadError, ElfLoadResult, ElfLoader};

use cache::{load_blob_cached, read_blob_from_cache, resolve_blob_id};
use image::{install_elf_image, install_stack_vma};
use parser::{default_interpreter_path, read_interpreter_path, validate_elf_header};
use registry::{
    clear_elf_segments, lookup_elf_blob, lookup_elf_segments, register_elf_blob,
    DynamicLoaderHandoff, ElfWalkAllocator,
};
use stack::{
    build_dynamic_handoff, build_initial_process_stack, map_stack_pages, write_stack_bytes,
    DYNAMIC_LOADER_HANDOFF_STACK_GAP,
};
use trace::{trace_blob, trace_path};

/// Implémentation du chargeur ELF via ExoFS.
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
        // ── 1. Résoudre le chemin ExoFS → BlobId ────────────────────────────
        trace_path(b"elf: load ", path);
        let blob_id = resolve_blob_id(path.as_bytes())?;
        trace_blob(b"elf: resolved ", &blob_id);
        let file_id = register_elf_blob(blob_id).ok_or(ElfLoadError::OutOfMemory)?;
        clear_elf_segments(file_id);

        // ── 2. Lire le blob depuis le cache ──────────────────────────────────
        let elf_data = read_blob_from_cache(&blob_id)?;

        // ── 3. Valider l'en-tête ELF et détecter PT_INTERP ───────────────────
        validate_elf_header(&elf_data)?;
        let interp_path = read_interpreter_path(&elf_data)?;
        let _ = default_interpreter_path; // conservé pour usage dynamique futur

        // ── 5. Créer le nouvel espace d'adressage ────────────────────────────
        let alloc = ElfWalkAllocator;
        let mut builder = PageTableBuilder::new(&alloc).map_err(|_| ElfLoadError::OutOfMemory)?;
        // Copier les entrées kernel (PML4[256..512]) pour que le fils puisse
        // atteindre le kernel lors des appels système
        unsafe {
            builder
                .copy_kernel_entries()
                .map_err(|_| ElfLoadError::OutOfMemory)?;
        }
        let pml4_phys = builder.pml4_phys();
        let child_as = Box::new(UserAddressSpace::new(pml4_phys, 0));

        // ── 6. Charger l'exécutable et éventuellement son interpréteur ───────
        let main_image = install_elf_image(&elf_data, file_id, &child_as, 0)?;
        let mut entry_point = main_image.entry_point;
        let mut entry_arg0 = 0u64;
        let mut interp_image = None;

        if let Some(interp) = interp_path {
            let interp_blob = resolve_blob_id(interp.as_bytes()).map_err(|err| {
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
            let image = install_elf_image(&interp_data, interp_file_id, &child_as, 0)?;
            entry_point = image.entry_point;
            interp_image = Some((interp, image));
        }

        // ── 7. brk_start = page suivante au-dessus du dernier segment ────────
        let brk_start =
            main_image.brk_end.saturating_add(PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
        child_as.init_heap_bounds(brk_start);

        // ── 8. Pile utilisateur : bootstrap eager minimal, plafond layout partagé ─
        const STACK_TOP: u64 = USER_STACK_TOP.as_u64();
        const STACK_SIZE: usize = USER_STACK_BOOTSTRAP_SIZE;
        let stack_size = STACK_SIZE;
        let stack_base = STACK_TOP.saturating_sub(stack_size as u64);

        let stack_frames = map_stack_pages(&mut builder, &alloc, stack_base, stack_size)?;
        install_stack_vma(&child_as, stack_base, stack_size)?;
        let executable_stack_top =
            build_initial_process_stack(&stack_frames, stack_base, STACK_TOP, argv, envp)?;
        if let Some((interp, image)) = interp_image {
            let handoff =
                build_dynamic_handoff(path, &interp, &main_image, &image, executable_stack_top);
            let handoff_bytes = unsafe {
                core::slice::from_raw_parts(
                    &handoff as *const DynamicLoaderHandoff as *const u8,
                    core::mem::size_of::<DynamicLoaderHandoff>(),
                )
            };
            let handoff_vaddr = (executable_stack_top
                .saturating_sub(DYNAMIC_LOADER_HANDOFF_STACK_GAP)
                .saturating_sub(handoff_bytes.len() as u64))
                & !15u64;
            write_stack_bytes(&stack_frames, stack_base, handoff_vaddr, handoff_bytes)?;
            entry_arg0 = handoff_vaddr;
        }

        // User ELF mappings are kept outside PML4[0]. Re-apply the low
        // supervisor window last so every userspace CR3 can safely run syscall
        // and IRQ paths while the kernel is still linked in low memory.
        builder
            .remap_low_kernel_identity()
            .map_err(|_| ElfLoadError::OutOfMemory)?;

        // ── 9. Finaliser ─────────────────────────────────────────────────────
        let child_cr3 = pml4_phys.as_u64();
        let addr_space_ptr = Box::into_raw(child_as) as usize;

        Ok(ElfLoadResult {
            entry_point,
            initial_stack_top: executable_stack_top,
            entry_arg0,
            tls_base: 0,
            tls_size: 0,
            brk_start,
            cr3: child_cr3,
            addr_space_ptr,
            signal_tcb_vaddr: 0,
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
        // FIX-25 : octets via cache (plus de relecture+rehash intégral par faute).
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

/// Instance statique du chargeur ELF.
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
}
