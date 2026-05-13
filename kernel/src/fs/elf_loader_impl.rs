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
use crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR;
use crate::memory::core::AllocError;
use crate::memory::core::PageFlags;
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::fault::demand_paging::FileFaultProvider;
use crate::memory::virt::page_table::builder::PageTableBuilder;
use crate::memory::virt::page_table::walker::FrameAllocatorForWalk;
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};
use crate::memory::virt::UserAddressSpace;
use crate::memory::{phys_to_virt, AllocFlags, Frame, VirtAddr, PAGE_SIZE};
use crate::process::lifecycle::exec::{ElfLoadError, ElfLoadResult, ElfLoader};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

const USER_ELF_BASE_MIN: u64 = 0x0000_0100_0000_0000;
const ELF_BLOB_REGISTRY_CAP: usize = 1024;

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
static ELF_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
#[inline(always)]
fn debug_byte(byte: u8) {
    unsafe {
        core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        debug_byte(byte);
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
fn debug_usize(mut value: usize) {
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    if value == 0 {
        debug_byte(b'0');
        return;
    }
    while value != 0 && len < buf.len() {
        buf[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len != 0 {
        len -= 1;
        debug_byte(buf[len]);
    }
}

#[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
fn debug_blob_prefix(blob_id: &BlobId) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in &blob_id.as_bytes()[..6] {
        debug_byte(HEX[(byte >> 4) as usize]);
        debug_byte(HEX[(byte & 0x0f) as usize]);
    }
}

#[inline]
fn trace_path(prefix: &[u8], path: &str) {
    let _ = (prefix, path);
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    {
        if ELF_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) >= 96 {
            return;
        }
        debug_write(prefix);
        debug_write(path.as_bytes());
        debug_write(b"\n");
    }
}

#[inline]
fn trace_blob(prefix: &[u8], blob_id: &BlobId) {
    let _ = (prefix, blob_id);
    #[cfg(all(target_arch = "x86_64", debug_assertions, exo_kernel_trace))]
    {
        if ELF_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) >= 96 {
            return;
        }
        debug_write(prefix);
        debug_blob_prefix(blob_id);
        debug_write(b" entries=");
        debug_usize(BLOB_CACHE.n_entries());
        debug_write(b"\n");
    }
}

struct ElfBlobRegistry {
    ids: [BlobId; ELF_BLOB_REGISTRY_CAP],
    count: usize,
}

impl ElfBlobRegistry {
    const fn new() -> Self {
        Self {
            ids: [BlobId::ZERO; ELF_BLOB_REGISTRY_CAP],
            count: 0,
        }
    }

    fn intern(&mut self, blob_id: BlobId) -> Option<u64> {
        for idx in 0..self.count {
            if self.ids[idx] == blob_id {
                return Some((idx + 1) as u64);
            }
        }
        if self.count >= ELF_BLOB_REGISTRY_CAP {
            return None;
        }
        let idx = self.count;
        self.ids[idx] = blob_id;
        self.count += 1;
        Some((idx + 1) as u64)
    }

    fn get(&self, file_id: u64) -> Option<BlobId> {
        let idx = file_id.checked_sub(1)? as usize;
        if idx >= self.count {
            return None;
        }
        Some(self.ids[idx])
    }
}

static ELF_BLOB_REGISTRY: Mutex<ElfBlobRegistry> = Mutex::new(ElfBlobRegistry::new());

fn register_elf_blob(blob_id: BlobId) -> Option<u64> {
    ELF_BLOB_REGISTRY.lock().intern(blob_id)
}

fn lookup_elf_blob(file_id: u64) -> Option<BlobId> {
    ELF_BLOB_REGISTRY.lock().get(file_id)
}

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
        trace_path(b"elf: load ", path);
        let blob_id = resolve_blob_id(path.as_bytes())?;
        trace_blob(b"elf: resolved ", &blob_id);
        let file_id = register_elf_blob(blob_id).ok_or(ElfLoadError::OutOfMemory)?;

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
            builder
                .copy_kernel_entries()
                .map_err(|_| ElfLoadError::OutOfMemory)?;
        }
        let pml4_phys = builder.pml4_phys();
        let child_as = Box::new(UserAddressSpace::new(pml4_phys, 0));

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
            validate_user_segment_range(p_vaddr, p_memsz)?;

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
            install_elf_vma(
                &child_as,
                p_vaddr,
                p_memsz,
                p_flags,
                p_offset as u64,
                file_id,
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
        install_stack_vma(&child_as, stack_base, stack_size)?;

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
            // `_start` est une fonction Rust normale atteinte par iretq, sans
            // adresse de retour pushée par `call`. L'ABI SysV veut donc
            // RSP % 16 == 8 à l'entrée pour que les frames internes restent
            // alignées 16 B avant les appels et les `movaps`.
            initial_stack_top: STACK_TOP - 8,
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
        let cached_data = BLOB_CACHE.get(&blob_id);
        let embedded_data = if cached_data.is_none() {
            crate::userspace_boot::embedded_payload_by_blob(blob_id)
        } else {
            None
        };
        let data = if let Some(ref cached) = cached_data {
            &cached[..]
        } else if let Some(embedded) = embedded_data {
            embedded
        } else {
            return Err(AllocError::InvalidParams);
        };
        let dst_virt = phys_to_virt(dest_frame.start_address());
        let dst =
            unsafe { core::slice::from_raw_parts_mut(dst_virt.as_u64() as *mut u8, PAGE_SIZE) };
        dst.fill(0);

        let start = file_offset as usize;
        if start >= data.len() {
            return Ok(());
        }
        let end = start.saturating_add(PAGE_SIZE).min(data.len());
        let len = end.saturating_sub(start);
        dst[..len].copy_from_slice(&data[start..end]);
        Ok(())
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
        .map_err(|_| {
            trace_path(
                b"elf: resolve failed ",
                core::str::from_utf8(path_bytes).unwrap_or("<non-utf8>"),
            );
            ElfLoadError::NotFound
        })
}

/// Lit le contenu complet d'un blob depuis BLOB_CACHE.
/// Retombe sur les payloads de boot embarqués si le cache ExoFS ne contient pas
/// encore l'entrée. Les binaires critiques du démarrage ne doivent pas dépendre
/// d'un état cache mutable pour `execve()`.
fn read_blob_from_cache(blob_id: &BlobId) -> Result<Arc<[u8]>, ElfLoadError> {
    if let Some(data) = BLOB_CACHE.get(blob_id) {
        trace_blob(b"elf: cache hit ", blob_id);
        return Ok(data);
    }

    trace_blob(b"elf: cache miss ", blob_id);
    if let Some(bytes) = crate::userspace_boot::embedded_payload_by_blob(*blob_id) {
        trace_blob(b"elf: embedded hit ", blob_id);
        let mut data = Vec::new();
        data.try_reserve_exact(bytes.len())
            .map_err(|_| ElfLoadError::OutOfMemory)?;
        data.extend_from_slice(bytes);
        return Ok(Arc::from(data.into_boxed_slice()));
    }

    Err(ElfLoadError::NotFound)
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

#[cfg(target_os = "none")]
fn kernel_low_identity_end() -> u64 {
    unsafe extern "C" {
        static __kernel_end: u8;
    }
    (&raw const __kernel_end) as u64
}

#[cfg(not(target_os = "none"))]
fn kernel_low_identity_end() -> u64 {
    0
}

fn validate_user_segment_range(p_vaddr: u64, p_memsz: usize) -> Result<(), ElfLoadError> {
    let Some(end_unaligned) = p_vaddr.checked_add(p_memsz as u64) else {
        return Err(ElfLoadError::InvalidElf);
    };
    let page_mask = PAGE_SIZE as u64 - 1;
    let start = p_vaddr & !page_mask;
    let end = end_unaligned.saturating_add(page_mask) & !page_mask;
    let kernel_start = KERNEL_LOAD_PHYS_ADDR;
    let kernel_end = kernel_low_identity_end();

    if start < USER_ELF_BASE_MIN {
        return Err(ElfLoadError::InvalidElf);
    }

    if kernel_end > kernel_start && start < kernel_end && end > kernel_start {
        return Err(ElfLoadError::InvalidElf);
    }

    Ok(())
}

fn elf_page_flags(p_flags: u32) -> PageFlags {
    let exec = (p_flags & 0x1) != 0;
    let write = (p_flags & 0x2) != 0;

    match (exec, write) {
        (true, true) => PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER,
        (true, false) => PageFlags::USER_CODE,
        (false, true) => PageFlags::USER_DATA,
        (false, false) => PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE,
    }
}

fn elf_vma_flags(p_flags: u32) -> VmaFlags {
    let mut flags = VmaFlags::NONE;
    if (p_flags & 0x4) != 0 {
        flags |= VmaFlags::READ;
    }
    if (p_flags & 0x2) != 0 {
        flags |= VmaFlags::WRITE;
    }
    if (p_flags & 0x1) != 0 {
        flags |= VmaFlags::EXEC;
    }
    flags
}

fn install_elf_vma(
    user_as: &UserAddressSpace,
    p_vaddr: u64,
    p_memsz: usize,
    p_flags: u32,
    file_offset: u64,
    file_id: u64,
) -> Result<(), ElfLoadError> {
    if p_memsz == 0 {
        return Ok(());
    }

    let start = p_vaddr & !(PAGE_SIZE as u64 - 1);
    let end_unaligned = p_vaddr.saturating_add(p_memsz as u64);
    let end = end_unaligned.saturating_add(PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
    let mut vma = Box::new(VmaDescriptor::new(
        VirtAddr::new(start),
        VirtAddr::new(end),
        elf_vma_flags(p_flags),
        elf_page_flags(p_flags),
        VmaBacking::File,
    ));
    vma.inode_id = file_id;
    vma.file_offset = file_offset & !(PAGE_SIZE as u64 - 1);
    let vma_ptr = Box::into_raw(vma);
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(ElfLoadError::OutOfMemory);
    }
    Ok(())
}

fn install_stack_vma(
    user_as: &UserAddressSpace,
    stack_base: u64,
    stack_size: usize,
) -> Result<(), ElfLoadError> {
    let vma = Box::new(VmaDescriptor::new(
        VirtAddr::new(stack_base),
        VirtAddr::new(stack_base.saturating_add(stack_size as u64)),
        VmaFlags::READ | VmaFlags::WRITE | VmaFlags::ANONYMOUS | VmaFlags::STACK,
        PageFlags::USER_DATA,
        VmaBacking::Anonymous,
    ));
    let vma_ptr = Box::into_raw(vma);
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(ElfLoadError::OutOfMemory);
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
    // Traduire les flags ELF en PageFlags x86-64.
    let page_flags = elf_page_flags(p_flags);

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
