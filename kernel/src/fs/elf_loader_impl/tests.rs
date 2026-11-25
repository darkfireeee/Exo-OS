//! Unit tests for `fs::elf_loader_impl`.

use crate::process::lifecycle::exec::ElfLoadError;

use super::loader::{
    choose_et_dyn_load_bias, intervals_overlap, INTERP_ASLR_BASE_ALT, INTERP_ASLR_BASE_PRIMARY,
    PIE_ASLR_BASE_ALT, PIE_ASLR_BASE_PRIMARY,
};
use super::parser::*;
use super::stack;

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
    buf[4] = 2;
    buf[5] = 1;
    buf[16..18].copy_from_slice(&e_type.to_le_bytes());
    buf[18..20].copy_from_slice(&0x3Eu16.to_le_bytes());
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
        build_auxv, AuxvParams, AT_BASE, AT_CAP_TOKEN, AT_EXECFN, AT_HWCAP2, AT_NULL, AT_PLATFORM,
        AT_SECURE, AT_SIGNAL_TCB,
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
