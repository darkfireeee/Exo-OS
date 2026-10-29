//! image.rs — Installation des segments ELF et VMAs dans l'espace d'adressage

use alloc::boxed::Box;
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor};
use crate::memory::virt::UserAddressSpace;
use crate::memory::{VirtAddr, PAGE_SIZE};
use crate::process::lifecycle::exec::ElfLoadError;

use super::parser::{
    elf_page_flags, elf_u16, elf_u32, elf_u64, elf_vma_flags, phdr_at, phdr_span,
    validate_load_segment_flags, validate_user_segment_range, PT_DYNAMIC, PT_LOAD, PT_PHDR, PT_TLS,
};
use super::registry::{register_elf_segment, LoadedElfImage};

pub fn install_elf_image(
    elf_data: &[u8],
    file_id: u64,
    child_as: &UserAddressSpace,
    load_bias: u64,
) -> Result<LoadedElfImage, ElfLoadError> {
    let entry_point = elf_u64(elf_data, 24).saturating_add(load_bias);
    let phent = elf_u16(elf_data, 54) as u64;
    let (e_phoff, e_phnum, e_phentsize) = phdr_span(elf_data)?;
    let mut brk_end = 0u64;
    let mut phdr_vaddr = 0u64;
    let mut dynamic_vaddr = 0u64;
    let mut dynamic_count = 0u64;
    let mut tls_vaddr = 0u64;
    let mut tls_offset = 0u64;
    let mut tls_filesz = 0u64;
    let mut tls_memsz = 0u64;
    let mut tls_align = 8u64;

    let mut i = 0usize;
    while i < e_phnum {
        let ph = phdr_at(elf_data, e_phoff, e_phentsize, i)?;
        let p_type = elf_u32(ph, 0);
        let p_flags = elf_u32(ph, 4);
        let p_offset = elf_u64(ph, 8) as usize;
        let p_vaddr = elf_u64(ph, 16).saturating_add(load_bias);
        let p_filesz = elf_u64(ph, 32) as usize;
        let p_memsz = elf_u64(ph, 40) as usize;
        let p_align = elf_u64(ph, 48);

        if p_type == PT_PHDR {
            phdr_vaddr = p_vaddr;
        } else if p_type == PT_DYNAMIC {
            dynamic_vaddr = p_vaddr;
            dynamic_count = (p_filesz / 16) as u64;
        } else if p_type == PT_TLS {
            if p_filesz > p_memsz || p_offset.saturating_add(p_filesz) > elf_data.len() {
                return Err(ElfLoadError::InvalidElf);
            }
            tls_vaddr = p_vaddr;
            tls_offset = p_offset as u64;
            tls_filesz = p_filesz as u64;
            tls_memsz = p_memsz as u64;
            tls_align = p_align.max(8);
        }

        if p_type != PT_LOAD {
            i += 1;
            continue;
        }
        if p_memsz == 0 {
            i += 1;
            continue;
        }
        validate_load_segment_flags(p_flags)?;
        if p_filesz > p_memsz || p_offset.saturating_add(p_filesz) > elf_data.len() {
            return Err(ElfLoadError::InvalidElf);
        }
        validate_user_segment_range(p_vaddr, p_memsz)?;
        register_elf_segment(file_id, p_offset as u64, p_filesz as u64)?;

        install_elf_vma(
            child_as,
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
        i += 1;
    }

    if phdr_vaddr == 0 {
        phdr_vaddr = load_bias.saturating_add(e_phoff as u64);
    }

    Ok(LoadedElfImage {
        entry_point,
        base: load_bias,
        phdr_vaddr,
        phnum: e_phnum as u64,
        phent,
        dynamic_vaddr,
        dynamic_count,
        brk_end,
        tls_vaddr,
        tls_offset,
        tls_filesz,
        tls_memsz,
        tls_align,
    })
}

pub fn install_elf_vma(
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

pub fn install_stack_vma(
    user_as: &UserAddressSpace,
    stack_base: u64,
    stack_size: usize,
) -> Result<(), ElfLoadError> {
    use crate::memory::core::PageFlags;
    use crate::memory::virt::vma::VmaFlags;

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

pub fn install_anon_rw_vma(
    user_as: &UserAddressSpace,
    start_vaddr: u64,
    size_bytes: usize,
    extra_flags: crate::memory::virt::vma::VmaFlags,
) -> Result<(), ElfLoadError> {
    use crate::memory::core::PageFlags;
    use crate::memory::virt::vma::VmaFlags;

    if size_bytes == 0 {
        return Ok(());
    }
    let vma = Box::new(VmaDescriptor::new(
        VirtAddr::new(start_vaddr),
        VirtAddr::new(start_vaddr.saturating_add(size_bytes as u64)),
        VmaFlags::READ | VmaFlags::WRITE | VmaFlags::ANONYMOUS | extra_flags,
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
