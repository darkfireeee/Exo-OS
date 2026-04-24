// kernel/src/fs/elf_loader_impl.rs
//
// Implémentation de ElfLoader pour charger les binaires ELF depuis ExoFS.
// CORRECTION P0-02 : débloque execve() en fournissant le chargement des binaires.
//
// Corrections appliquées (FIX-4) :
//   - resolve_blob_id : appelle resolve_path_to_blob() au lieu de toujours retourner NotFound
//   - Lecture cache   : BLOB_CACHE.get(&BlobId) — API correcte (plus get_blob_data)
//   - map_elf_segment : alloc frame + copie données + map_page() via PageTableBuilder
//   - map_stack_pages : alloc frames ZEROED + map_page() via PageTableBuilder
//   - ElfLoadError    : InvalidElf/UnsupportedArch (variants qui existent dans l'enum)

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::syscall::path_resolve::resolve_path_to_blob;
use crate::memory::core::AllocError;
use crate::memory::core::PageFlags;
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::page_table::builder::PageTableBuilder;
use crate::memory::virt::page_table::walker::FrameAllocatorForWalk;
use crate::memory::virt::UserAddressSpace;
use crate::memory::{phys_to_virt, AllocFlags, Frame, VirtAddr, PAGE_SIZE};
use crate::process::lifecycle::exec::{ElfLoadError, ElfLoadResult, ElfLoader};
use alloc::boxed::Box;

// ─────────────────────────────────────────────────────────────────────────────
// Allocateur de frames pour les walks de table de pages de l'ELF loader
// ─────────────────────────────────────────────────────────────────────────────

struct ElfWalkAllocator;

impl FrameAllocatorForWalk for ElfWalkAllocator {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        buddy::alloc_pages(0, flags)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = buddy::free_pages(frame, 0);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Implémentation du chargeur ELF
// ─────────────────────────────────────────────────────────────────────────────

/// Implémentation du chargeur ELF via ExoFS.
pub struct ExoFsElfLoader;

unsafe impl Send for ExoFsElfLoader {}
unsafe impl Sync for ExoFsElfLoader {}

impl ElfLoader for ExoFsElfLoader {
    fn load_elf(
        &self,
        path: &str,
        _argv: &[&str],
        _envp: &[&str],
        _cr3_in: u64,
    ) -> Result<ElfLoadResult, ElfLoadError> {
        // ── 1. Résoudre le chemin ExoFS → BlobId ────────────────────────────
        let blob_id = resolve_blob_id(path.as_bytes())?;

        // ── 2. Lire le blob depuis le cache ──────────────────────────────────
        let elf_data = read_blob_from_cache(&blob_id)?;

        // ── 3. Valider l'en-tête ELF ─────────────────────────────────────────
        validate_elf_header(&elf_data)?;

        // ── 4. Extraire les champs de l'en-tête ELF64 ────────────────────────
        // Offsets spec ELF64 :
        //   [24..32] e_entry     adresse d'entrée
        //   [32..40] e_phoff     offset table program headers
        //   [56..58] e_phnum     nombre de program headers
        let entry_point = u64::from_le_bytes(elf_data[24..32].try_into().unwrap());
        let e_phoff = u64::from_le_bytes(elf_data[32..40].try_into().unwrap()) as usize;
        let e_phnum = u16::from_le_bytes([elf_data[56], elf_data[57]]) as usize;

        // ── 5. Créer le nouvel espace d'adressage ────────────────────────────
        let alloc = ElfWalkAllocator;
        let mut builder = PageTableBuilder::new(&alloc).map_err(|_| ElfLoadError::OutOfMemory)?;
        // Copier les entrées kernel (PML4[256..512]) pour que le fils puisse
        // atteindre le kernel lors des appels système
        unsafe {
            builder.copy_kernel_entries();
        }
        let pml4_phys = builder.pml4_phys();

        // ── 6. Charger les segments PT_LOAD ──────────────────────────────────
        // Chaque program header ELF64 = 56 octets
        const PHDR_SIZE: usize = 56;
        let mut brk_end: u64 = 0;

        for i in 0..e_phnum {
            let ph_off = e_phoff.saturating_add(i.saturating_mul(PHDR_SIZE));
            let ph_end = ph_off.saturating_add(PHDR_SIZE);
            if ph_end > elf_data.len() {
                return Err(ElfLoadError::InvalidElf);
            }
            let ph = &elf_data[ph_off..ph_end];

            // p_type [0..4] — seul PT_LOAD (1) nous intéresse
            let p_type = u32::from_le_bytes([ph[0], ph[1], ph[2], ph[3]]);
            if p_type != 1 {
                continue;
            }

            // Champs ELF64 program header :
            //  [0..4]   p_type
            //  [4..8]   p_flags   PF_X=1, PF_W=2, PF_R=4
            //  [8..16]  p_offset  offset dans le fichier ELF
            //  [16..24] p_vaddr   adresse virtuelle de destination
            //  [32..40] p_filesz  taille des données dans le fichier
            //  [40..48] p_memsz   taille en mémoire (> p_filesz = .bss zeroed)
            let p_flags = u32::from_le_bytes([ph[4], ph[5], ph[6], ph[7]]);
            let p_offset = u64::from_le_bytes(ph[8..16].try_into().unwrap()) as usize;
            let p_vaddr = u64::from_le_bytes(ph[16..24].try_into().unwrap());
            let p_filesz = u64::from_le_bytes(ph[32..40].try_into().unwrap()) as usize;
            let p_memsz = u64::from_le_bytes(ph[40..48].try_into().unwrap()) as usize;

            if p_memsz == 0 {
                continue;
            }
            if p_offset.saturating_add(p_filesz) > elf_data.len() {
                return Err(ElfLoadError::InvalidElf);
            }

            map_elf_segment(
                &mut builder,
                &alloc,
                &elf_data,
                p_offset,
                p_vaddr,
                p_filesz,
                p_memsz,
                p_flags,
            )?;

            let seg_end = p_vaddr.saturating_add(p_memsz as u64);
            if seg_end > brk_end {
                brk_end = seg_end;
            }
        }

        // ── 7. brk_start = page suivante au-dessus du dernier segment ────────
        let brk_start = brk_end.saturating_add(PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

        // ── 8. Pile utilisateur : 8 pages, sommet à 0x7FFF_FFFF_0000 ─────────
        const STACK_TOP: u64 = 0x0000_7fff_ffff_0000u64;
        const STACK_PAGES: usize = 8;
        let stack_size = STACK_PAGES * PAGE_SIZE;
        let stack_base = STACK_TOP.saturating_sub(stack_size as u64);

        map_stack_pages(&mut builder, &alloc, stack_base, stack_size)?;

        // ── 9. Finaliser ─────────────────────────────────────────────────────
        let child_cr3 = pml4_phys.as_u64();
        let child_as = Box::new(UserAddressSpace::new(pml4_phys, 0));
        let addr_space_ptr = Box::into_raw(child_as) as usize;

        Ok(ElfLoadResult {
            entry_point,
            // RSP initial : 16 octets sous le sommet, aligné 16 B (ABI x86-64)
            initial_stack_top: (STACK_TOP - 16) & !0xF,
            tls_base: 0,
            tls_size: 0,
            brk_start,
            cr3: child_cr3,
            addr_space_ptr,
            signal_tcb_vaddr: 0,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers privés
// ─────────────────────────────────────────────────────────────────────────────

/// Résout un chemin ExoFS en BlobId.
/// Délègue à resolve_path_to_blob() (pub(crate)) qui fait Blake3 du chemin canonique.
fn resolve_blob_id(path_bytes: &[u8]) -> Result<BlobId, ElfLoadError> {
    resolve_path_to_blob(path_bytes, 0)
        .map(|r| BlobId(r.blob_id))
        .map_err(|_| ElfLoadError::NotFound)
}

/// Lit le contenu complet d'un blob depuis BLOB_CACHE.
/// Retourne NotFound si le blob n'est pas présent dans le cache.
fn read_blob_from_cache(blob_id: &BlobId) -> Result<alloc::boxed::Box<[u8]>, ElfLoadError> {
    BLOB_CACHE.get(blob_id).ok_or(ElfLoadError::NotFound)
}

/// Valide magic, classe ELF et architecture.
fn validate_elf_header(data: &[u8]) -> Result<(), ElfLoadError> {
    if data.len() < 64 {
        return Err(ElfLoadError::InvalidElf);
    }
    if &data[0..4] != b"\x7FELF" {
        return Err(ElfLoadError::InvalidElf);
    }
    // EI_CLASS = 2 → 64-bit
    if data[4] != 2 {
        return Err(ElfLoadError::UnsupportedArch);
    }
    // EI_DATA = 1 → little-endian
    if data[5] != 1 {
        return Err(ElfLoadError::UnsupportedArch);
    }
    // e_machine = 0x3E → EM_X86_64
    let e_machine = u16::from_le_bytes([data[18], data[19]]);
    if e_machine != 0x3E {
        return Err(ElfLoadError::UnsupportedArch);
    }
    Ok(())
}

/// Mappe un segment ELF PT_LOAD dans le nouvel espace d'adressage.
///
/// Pour chaque page couverte par [p_vaddr .. p_vaddr + p_memsz) :
///   1. Alloue un frame physique ZEROED (les octets hors p_filesz restent à 0 = .bss)
///   2. Copie la portion du fichier correspondante via le physmap kernel
///   3. Mappe le frame dans le PageTableBuilder avec les flags ELF traduits en PageFlags
#[allow(clippy::too_many_arguments)]
fn map_elf_segment<A: FrameAllocatorForWalk>(
    builder: &mut PageTableBuilder<A>,
    _alloc: &A,
    elf_data: &[u8],
    p_offset: usize,
    p_vaddr: u64,
    p_filesz: usize,
    p_memsz: usize,
    p_flags: u32,
) -> Result<(), ElfLoadError> {
    // Traduire les flags ELF en PageFlags x86-64
    // PF_X=1, PF_W=2, PF_R=4
    let exec = (p_flags & 0x1) != 0;
    let write = (p_flags & 0x2) != 0;

    let page_flags = match (exec, write) {
        (true, true) => PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER,
        (true, false) => PageFlags::USER_CODE, // PRESENT | USER (no NX)
        (false, true) => PageFlags::USER_DATA, // PRESENT | WRITABLE | USER | NX
        (false, false) => PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE,
    };

    // Aligner vaddr sur la page (un segment peut commencer au milieu d'une page)
    let vaddr_page = p_vaddr & !(PAGE_SIZE as u64 - 1);
    let page_offset = (p_vaddr - vaddr_page) as usize; // décalage dans la 1ère page
    let total_size = page_offset + p_memsz;
    let n_pages = total_size.saturating_add(PAGE_SIZE - 1) / PAGE_SIZE;

    for page_idx in 0..n_pages {
        let frame =
            buddy::alloc_pages(0, AllocFlags::ZEROED).map_err(|_| ElfLoadError::OutOfMemory)?;

        // Plage de bytes du segment à copier dans cette page :
        //   seg_byte_start/end : relatif au début du segment (p_vaddr)
        //   file_start/end     : relatif au début du blob (p_offset)
        let seg_byte_start = (page_idx * PAGE_SIZE).saturating_sub(page_offset);
        let seg_byte_end = ((page_idx + 1) * PAGE_SIZE)
            .saturating_sub(page_offset)
            .min(p_filesz);

        if seg_byte_start < seg_byte_end {
            let file_start = p_offset.saturating_add(seg_byte_start);
            let file_end = p_offset.saturating_add(seg_byte_end);
            // Offset d'écriture dans la page (0 pour toutes les pages sauf la 1ère)
            let dst_off = if page_idx == 0 { page_offset } else { 0 };
            let copy_len = seg_byte_end - seg_byte_start;

            if file_end <= elf_data.len() {
                // Accès à la frame via le physmap direct kernel (PHYS_MAP_BASE + phys)
                let frame_virt = phys_to_virt(frame.start_address());
                let dst = unsafe {
                    core::slice::from_raw_parts_mut(frame_virt.as_u64() as *mut u8, PAGE_SIZE)
                };
                dst[dst_off..dst_off + copy_len].copy_from_slice(&elf_data[file_start..file_end]);
            }
        }

        let virt = VirtAddr::new(vaddr_page.saturating_add((page_idx * PAGE_SIZE) as u64));
        builder
            .map_page(virt, frame, page_flags)
            .map_err(|_| ElfLoadError::OutOfMemory)?;
    }

    Ok(())
}

/// Alloue et mappe les pages de pile utilisateur (toutes ZEROED, USER_DATA).
fn map_stack_pages<A: FrameAllocatorForWalk>(
    builder: &mut PageTableBuilder<A>,
    _alloc: &A,
    stack_base: u64,
    stack_size: usize,
) -> Result<(), ElfLoadError> {
    let n_pages = stack_size / PAGE_SIZE;
    for page_idx in 0..n_pages {
        let frame =
            buddy::alloc_pages(0, AllocFlags::ZEROED).map_err(|_| ElfLoadError::OutOfMemory)?;
        let virt = VirtAddr::new(stack_base.saturating_add((page_idx * PAGE_SIZE) as u64));
        builder
            .map_page(virt, frame, PageFlags::USER_DATA)
            .map_err(|_| ElfLoadError::OutOfMemory)?;
    }
    Ok(())
}

/// Instance statique du chargeur ELF.
pub static EXO_ELF_LOADER: ExoFsElfLoader = ExoFsElfLoader;
