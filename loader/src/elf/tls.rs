//! Variant II x86-64 ELF TLS, `GenericTcb` (`%fs:0`), dynamic `DTV`, `__tls_get_addr`, and `TLSDESC` helpers.

use alloc::vec::Vec;
use core::mem::offset_of;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::alloc_mmap::SpinLock;
use crate::syscall::{
    sys_arch_set_fs, sys_mmap, sys_munmap, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE,
};

const PAGE_SIZE: usize = 4096;
/// Extra static TLS space reserved below initial static blocks so `dlopen()` can still
/// place small `DF_STATIC_TLS` / `TPOFF64` modules when present.
const STATIC_TLS_SPARE_BYTES: usize = 4096;
/// Header capacity stored before dynamic `dtv` array: `[alloc_bytes, spare_top_or_reserved, generation]`.
const DTV_META_WORDS: usize = 3;

/// Unified Variant II `%fs:0` TCB matching kernel, `exo-rt`, `std`, and `musl-exo`.
#[repr(C)]
pub struct GenericTcb {
    pub tcb_ptr: *mut Self,
    pub dtv: *mut usize,
    pub prev: *mut u8,
    pub next: *mut u8,
    pub sysinfo: usize,
    pub canary: usize,
    pub tid: i32,
    pub errno: i32,
    pub tls_end: *mut u8,
    pub tls_len: usize,
    pub tcb_len: usize,
    pub os_specific: *mut u8,
}

const _: () = {
    assert!(offset_of!(GenericTcb, tcb_ptr) == 0);
    assert!(offset_of!(GenericTcb, dtv) == 8);
    assert!(offset_of!(GenericTcb, prev) == 16);
    assert!(offset_of!(GenericTcb, next) == 24);
    assert!(offset_of!(GenericTcb, sysinfo) == 32);
    assert!(offset_of!(GenericTcb, canary) == 40);
    assert!(offset_of!(GenericTcb, tid) == 48);
    assert!(offset_of!(GenericTcb, errno) == 52);
    assert!(offset_of!(GenericTcb, tls_end) == 56);
    assert!(offset_of!(GenericTcb, tls_len) == 64);
    assert!(offset_of!(GenericTcb, tcb_len) == 72);
    assert!(offset_of!(GenericTcb, os_specific) == 80);
    assert!(core::mem::size_of::<GenericTcb>() == 88);
};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TlsIndex {
    pub ti_module: usize,
    pub ti_offset: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct TlsModuleDesc {
    pub mod_id: usize,
    pub init_image: usize,
    pub filesz: usize,
    pub memsz: usize,
    pub align: usize,
    pub vaddr_mod_align: usize,
    /// Negative byte offset from `%fs:0` for static TLS modules, or `0` if dynamic-only.
    pub static_tpoff: isize,
    pub is_static: bool,
}

#[derive(Clone, Copy, Debug)]
struct DynTlsAllocRecord {
    dtv_alloc_base: usize,
    dtv_alloc_len: usize,
}

pub struct TlsState {
    modules: Vec<TlsModuleDesc>,
    static_cursor: usize,
    max_static_align: usize,
    static_sealed: bool,
    generation: usize,
    dyn_dtv_records: Vec<(usize, DynTlsAllocRecord)>,
}

impl TlsState {
    const fn new() -> Self {
        Self {
            modules: Vec::new(),
            static_cursor: 0,
            max_static_align: 16,
            static_sealed: false,
            generation: 1,
            dyn_dtv_records: Vec::new(),
        }
    }
}

pub static TLS_STATE: SpinLock<TlsState> = SpinLock::new(TlsState::new());
static CURRENT_CANARY: AtomicUsize = AtomicUsize::new(0);
static CURRENT_SIGNAL_TCB: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
pub unsafe fn current_tcb_ptr() -> *mut GenericTcb {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        let ptr: *mut GenericTcb;
        unsafe {
            core::arch::asm!(
                "mov {}, fs:[0]",
                out(reg) ptr,
                options(nostack, preserves_flags, readonly)
            );
        }
        ptr
    }
    #[cfg(not(all(target_arch = "x86_64", not(test))))]
    {
        ptr::null_mut()
    }
}

/// Computes `(next_cursor, negative_tpoff)` for a static Variant II TLS block below `%fs:0`.
#[inline]
pub fn compute_variant2_slot(
    cursor: usize,
    memsz: usize,
    align: usize,
    vaddr_mod_align: usize,
) -> Option<(usize, isize)> {
    let align = if align <= 1 { 1 } else { align.next_power_of_two() };
    let raw_end = cursor.checked_add(memsz)?;
    let misalign = (0usize.wrapping_sub(raw_end).wrapping_sub(vaddr_mod_align)) & (align - 1);
    let next_cursor = raw_end.checked_add(misalign)?;
    let tpoff = -(next_cursor as isize);
    Some((next_cursor, tpoff))
}

/// Registers a `PT_TLS` template. Modules registered before `seal_initial_tls()` belong
/// to the static TLS image placed directly below `%fs:0`. Later `dlopen()` modules allocate
/// lazily per thread in `DTV` (or carve from spare space when `static_tls_required`).
pub fn register_tls_module(
    init_image: usize,
    filesz: usize,
    memsz: usize,
    align: usize,
    p_vaddr: usize,
    static_tls_required: bool,
) -> Result<(usize, isize), i32> {
    if memsz == 0 {
        return Ok((0, 0));
    }
    let align = if align <= 1 { 1 } else { align.next_power_of_two() };
    let vaddr_mod_align = p_vaddr & (align - 1);

    let mut st = TLS_STATE.lock();
    let mod_id = st.modules.len() + 1;
    let (is_static, static_tpoff) = if !st.static_sealed {
        let Some((next_cursor, tpoff)) =
            compute_variant2_slot(st.static_cursor, memsz, align, vaddr_mod_align)
        else {
            return Err(12); // ENOMEM
        };
        st.static_cursor = next_cursor;
        if align > st.max_static_align {
            st.max_static_align = align;
        }
        (true, tpoff)
    } else if static_tls_required {
        let Some((next_cursor, tpoff)) =
            compute_variant2_slot(st.static_cursor, memsz, align, vaddr_mod_align)
        else {
            return Err(12);
        };
        if next_cursor > STATIC_TLS_SPARE_BYTES && align > PAGE_SIZE {
            return Err(22); // EINVAL
        }
        st.static_cursor = next_cursor;
        (true, tpoff)
    } else {
        (false, 0)
    };

    st.modules.push(TlsModuleDesc {
        mod_id,
        init_image,
        filesz: filesz.min(memsz),
        memsz,
        align,
        vaddr_mod_align,
        static_tpoff,
        is_static,
    });
    st.generation = st.generation.wrapping_add(1);
    Ok((mod_id, static_tpoff))
}

/// Seals the initial static TLS block size before main execution starts and builds `%fs:0`
/// when dynamic initial modules introduced additional `PT_TLS` segments beyond what the kernel set up.
pub unsafe fn finalize_initial_thread_tls() -> Result<(), i32> {
    let kernel_tcb = unsafe { current_tcb_ptr() };
    let (canary, sig_tcb, tid) = if !kernel_tcb.is_null() {
        unsafe {
            (
                (*kernel_tcb).canary,
                (*kernel_tcb).os_specific as usize,
                (*kernel_tcb).tid,
            )
        }
    } else {
        (0, 0, 0)
    };
    CURRENT_CANARY.store(canary, Ordering::Release);
    CURRENT_SIGNAL_TCB.store(sig_tcb, Ordering::Release);

    let (need_rebuild, modules_snapshot, static_cursor, max_align, generation) = {
        let mut st = TLS_STATE.lock();
        st.static_sealed = true;
        (
            !kernel_tcb.is_null() || !st.modules.is_empty(),
            st.modules.clone(),
            st.static_cursor,
            st.max_static_align,
            st.generation,
        )
    };

    if !need_rebuild {
        return Ok(());
    }

    let mut raw_base = 0usize;
    let mut raw_len = 0usize;
    let mut fs_base = 0u64;
    allocate_thread_tcb_from_snapshot(
        &modules_snapshot,
        static_cursor,
        max_align,
        generation,
        canary,
        sig_tcb,
        tid,
        &mut raw_base,
        &mut raw_len,
        &mut fs_base,
    )?;

    unsafe {
        sys_arch_set_fs(fs_base)?;
    }
    let new_tcb = fs_base as *mut GenericTcb;
    if !kernel_tcb.is_null() {
        unsafe {
            (*new_tcb).errno = (*kernel_tcb).errno;
        }
    }
    Ok(())
}

fn allocate_thread_tcb_from_snapshot(
    modules: &[TlsModuleDesc],
    static_cursor: usize,
    max_align: usize,
    generation: usize,
    canary: usize,
    sig_tcb: usize,
    tid: i32,
    out_raw_base: &mut usize,
    out_raw_len: &mut usize,
    out_fs_base: &mut u64,
) -> Result<(), i32> {
    let static_span = static_cursor + STATIC_TLS_SPARE_BYTES;
    let base_align = PAGE_SIZE.max(max_align);
    let aligned_block_span = if static_span == 0 {
        PAGE_SIZE
    } else {
        (static_span + base_align - 1) & !(base_align - 1)
    };
    let extra_align_pad = base_align.saturating_sub(PAGE_SIZE);
    let total_modules = modules.len();
    let dtv_words = 1 + total_modules + 8; // dtv[0] + slots 1..=N + headroom
    let dtv_bytes = (DTV_META_WORDS + dtv_words) * core::mem::size_of::<usize>();
    let tcb_pages = ((128 + dtv_bytes) + PAGE_SIZE - 1) / PAGE_SIZE;
    let alloc_len = aligned_block_span + extra_align_pad + (tcb_pages * PAGE_SIZE);

    let raw_base = unsafe {
        sys_mmap(
            0,
            alloc_len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )?
    };
    let raw_end_for_block = raw_base + aligned_block_span;
    let tls_base = if max_align <= PAGE_SIZE {
        raw_end_for_block
    } else {
        (raw_end_for_block + max_align - 1) & !(max_align - 1)
    };

    let dtv_meta = (tls_base + 128) as *mut usize;
    let dtv_ptr = unsafe { dtv_meta.add(DTV_META_WORDS) };
    unsafe {
        *dtv_meta.add(0) = 0; // Inline DTV with TCB, freed together in free_thread_tls
        *dtv_meta.add(1) = dtv_words;
        *dtv_meta.add(2) = generation;
        *dtv_ptr = total_modules;
    }

    for m in modules {
        if m.is_static && m.mod_id > 0 {
            let block_ptr = (tls_base as isize + m.static_tpoff) as *mut u8;
            if m.filesz > 0 && m.init_image != 0 {
                unsafe {
                    ptr::copy_nonoverlapping(m.init_image as *const u8, block_ptr, m.filesz);
                }
            }
            unsafe {
                *dtv_ptr.add(m.mod_id) = block_ptr as usize;
            }
        }
    }

    let tcb_ptr = tls_base as *mut GenericTcb;
    unsafe {
        (*tcb_ptr).tcb_ptr = tcb_ptr;
        (*tcb_ptr).dtv = dtv_ptr;
        (*tcb_ptr).prev = ptr::null_mut();
        (*tcb_ptr).next = ptr::null_mut();
        (*tcb_ptr).sysinfo = 0;
        (*tcb_ptr).canary = canary;
        (*tcb_ptr).tid = tid;
        (*tcb_ptr).errno = 0;
        (*tcb_ptr).tls_end = tls_base as *mut u8;
        (*tcb_ptr).tls_len = alloc_len;
        (*tcb_ptr).tcb_len = tcb_pages * PAGE_SIZE;
        (*tcb_ptr).os_specific = sig_tcb as *mut u8;
    }

    *out_raw_base = raw_base;
    *out_raw_len = alloc_len;
    *out_fs_base = tls_base as u64;
    Ok(())
}

/// Hook invoked by `std::thread::spawn` (`runtime.rs`) to allocate full TLS+DTV for new threads.
#[no_mangle]
pub unsafe extern "C" fn __exo_dl_alloc_thread_tls(
    parent_tcb: *const u8,
    out_raw_base: *mut usize,
    out_raw_len: *mut usize,
    out_fs_base: *mut u64,
) -> i32 {
    if out_raw_base.is_null() || out_raw_len.is_null() || out_fs_base.is_null() {
        return -22; // -EINVAL
    }
    let canary = if !parent_tcb.is_null() {
        unsafe { (*(parent_tcb as *const GenericTcb)).canary }
    } else {
        CURRENT_CANARY.load(Ordering::Acquire)
    };
    let sig_tcb = if !parent_tcb.is_null() {
        unsafe { (*(parent_tcb as *const GenericTcb)).os_specific as usize }
    } else {
        CURRENT_SIGNAL_TCB.load(Ordering::Acquire)
    };
    let (modules, static_cursor, max_align, generation) = {
        let st = TLS_STATE.lock();
        (
            st.modules.clone(),
            st.static_cursor,
            st.max_static_align,
            st.generation,
        )
    };
    let mut rb = 0usize;
    let mut rl = 0usize;
    let mut fb = 0u64;
    match allocate_thread_tcb_from_snapshot(
        &modules,
        static_cursor,
        max_align,
        generation,
        canary,
        sig_tcb,
        0,
        &mut rb,
        &mut rl,
        &mut fb,
    ) {
        Ok(()) => {
            unsafe {
                *out_raw_base = rb;
                *out_raw_len = rl;
                *out_fs_base = fb;
            }
            0
        }
        Err(e) => -e,
    }
}

/// Hook invoked on thread teardown to release dynamic `DTV` module allocations.
#[no_mangle]
pub unsafe extern "C" fn __exo_dl_free_thread_tls(tcb_ptr: *mut u8) {
    if tcb_ptr.is_null() {
        return;
    }
    let tcb = unsafe { &mut *(tcb_ptr as *mut GenericTcb) };
    let dtv_ptr = tcb.dtv;
    if dtv_ptr.is_null() {
        return;
    }
    let cnt = unsafe { *dtv_ptr };
    let (modules, dyn_rec) = {
        let mut st = TLS_STATE.lock();
        let rec_pos = st
            .dyn_dtv_records
            .iter()
            .position(|(owner, _)| *owner == tcb_ptr as usize);
        let rec = rec_pos.map(|idx| st.dyn_dtv_records.swap_remove(idx).1);
        (st.modules.clone(), rec)
    };
    for m in modules {
        if !m.is_static && m.mod_id <= cnt {
            let block_ptr = unsafe { *dtv_ptr.add(m.mod_id) };
            if block_ptr != 0 {
                let hdr = (block_ptr - core::mem::size_of::<usize>() * 2) as *const usize;
                let alloc_base = unsafe { *hdr };
                let alloc_len = unsafe { *hdr.add(1) };
                if alloc_base != 0 && alloc_len > 0 {
                    unsafe {
                        let _ = sys_munmap(alloc_base, alloc_len);
                    }
                }
            }
        }
    }
    if let Some(rec) = dyn_rec {
        if rec.dtv_alloc_base != 0 && rec.dtv_alloc_len > 0 {
            unsafe {
                let _ = sys_munmap(rec.dtv_alloc_base, rec.dtv_alloc_len);
            }
        }
    }
}

/// Slow path for `__tls_get_addr`: grows DTV and allocates dynamic module TLS block.
#[inline(never)]
unsafe fn tls_get_addr_slow(ti: &TlsIndex) -> *mut u8 {
    let tcb = unsafe { current_tcb_ptr() };
    if tcb.is_null() || ti.ti_module == 0 {
        return ptr::null_mut();
    }
    let (mod_desc, total_mods, generation) = {
        let st = TLS_STATE.lock();
        let desc = st.modules.iter().find(|m| m.mod_id == ti.ti_module).copied();
        (desc, st.modules.len(), st.generation)
    };
    let Some(m) = mod_desc else {
        return ptr::null_mut();
    };

    let mut dtv = unsafe { (*tcb).dtv };
    let cur_cnt = if dtv.is_null() {
        0
    } else {
        unsafe { *dtv }
    };
    if ti.ti_module > cur_cnt {
        // Allocate an expanded DTV via mmap so existing entries remain valid.
        let new_cap = total_mods.max(ti.ti_module) + 8;
        let bytes = ((DTV_META_WORDS + 1 + new_cap) * core::mem::size_of::<usize>() + PAGE_SIZE - 1)
            & !(PAGE_SIZE - 1);
        let Ok(new_map) = (unsafe {
            sys_mmap(
                0,
                bytes,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        }) else {
            return ptr::null_mut();
        };
        let meta = new_map as *mut usize;
        let new_dtv = unsafe { meta.add(DTV_META_WORDS) };
        unsafe {
            *meta.add(0) = bytes;
            *meta.add(1) = new_cap;
            *meta.add(2) = generation;
            *new_dtv = total_mods.max(ti.ti_module);
            if !dtv.is_null() {
                for i in 1..=cur_cnt {
                    *new_dtv.add(i) = *dtv.add(i);
                }
            }
            (*tcb).dtv = new_dtv;
        }
        let old_rec = {
            let mut st = TLS_STATE.lock();
            let old = st
                .dyn_dtv_records
                .iter()
                .position(|(k, _)| *k == tcb as usize)
                .map(|idx| core::mem::replace(&mut st.dyn_dtv_records[idx].1, DynTlsAllocRecord {
                    dtv_alloc_base: new_map,
                    dtv_alloc_len: bytes,
                }));
            if old.is_none() {
                st.dyn_dtv_records.push((
                    tcb as usize,
                    DynTlsAllocRecord {
                        dtv_alloc_base: new_map,
                        dtv_alloc_len: bytes,
                    },
                ));
            }
            old
        };
        if let Some(prev) = old_rec {
            if prev.dtv_alloc_base != 0 && prev.dtv_alloc_len > 0 {
                unsafe {
                    let _ = sys_munmap(prev.dtv_alloc_base, prev.dtv_alloc_len);
                }
            }
        }
        dtv = new_dtv;
    }

    let slot = unsafe { *dtv.add(ti.ti_module) };
    if slot != 0 {
        return (slot + ti.ti_offset) as *mut u8;
    }

    // Allocate per-module dynamic TLS block (with [alloc_base, alloc_len] header right before payload).
    let align = m.align.max(16);
    let hdr_bytes = core::mem::size_of::<usize>() * 2;
    let needed = hdr_bytes + align + m.memsz;
    let map_len = (needed + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let Ok(raw) = (unsafe {
        sys_mmap(
            0,
            map_len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    }) else {
        return ptr::null_mut();
    };
    let payload = ((raw + hdr_bytes) + align - 1) & !(align - 1);
    let hdr = (payload - hdr_bytes) as *mut usize;
    unsafe {
        *hdr = raw;
        *hdr.add(1) = map_len;
        if m.filesz > 0 && m.init_image != 0 {
            ptr::copy_nonoverlapping(m.init_image as *const u8, payload as *mut u8, m.filesz);
        }
        *dtv.add(ti.ti_module) = payload;
    }
    (payload + ti.ti_offset) as *mut u8
}

/// SysV x86-64 `__tls_get_addr(TlsIndex* ti)`.
#[no_mangle]
pub unsafe extern "C" fn __tls_get_addr(ti: *const TlsIndex) -> *mut u8 {
    if ti.is_null() {
        return ptr::null_mut();
    }
    let ti_ref = unsafe { &*ti };
    let tcb = unsafe { current_tcb_ptr() };
    if !tcb.is_null() {
        let dtv = unsafe { (*tcb).dtv };
        if !dtv.is_null() {
            let cnt = unsafe { *dtv };
            if ti_ref.ti_module > 0 && ti_ref.ti_module <= cnt {
                let base = unsafe { *dtv.add(ti_ref.ti_module) };
                if base != 0 {
                    return (base + ti_ref.ti_offset) as *mut u8;
                }
            }
        }
    }
    unsafe { tls_get_addr_slow(ti_ref) }
}

/// Fast static `TLSDESC` resolver: `RAX` holds pointer to `[resolver, tpoff_val]`.
#[cfg(all(target_arch = "x86_64", not(test)))]
#[unsafe(naked)]
pub unsafe extern "C" fn exo_tlsdesc_static() -> isize {
    core::arch::naked_asm!("mov rax, [rax + 8]", "ret");
}

#[cfg(not(all(target_arch = "x86_64", not(test))))]
pub unsafe extern "C" fn exo_tlsdesc_static() -> isize {
    0
}

#[repr(C)]
pub struct TlsDescDynamicArg {
    pub mod_id: usize,
    pub offset: usize,
}

#[no_mangle]
pub unsafe extern "C" fn exo_tlsdesc_dynamic_helper(arg: *const TlsDescDynamicArg) -> isize {
    if arg.is_null() {
        return 0;
    }
    let a = unsafe { &*arg };
    let ti = TlsIndex {
        ti_module: a.mod_id,
        ti_offset: a.offset,
    };
    let addr = unsafe { __tls_get_addr(&ti) } as usize;
    let fs_base = unsafe { current_tcb_ptr() } as usize;
    (addr as isize).wrapping_sub(fs_base as isize)
}

#[cfg(all(target_arch = "x86_64", not(test)))]
#[unsafe(naked)]
pub unsafe extern "C" fn exo_tlsdesc_dynamic() -> isize {
    // TLSDESC ABI: input RAX = &slot ([fn_ptr, *mut TlsDescDynamicArg]), preserves all regs except RAX & RFLAGS.
    core::arch::naked_asm!(
        "sub rsp, 8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "mov rdi, [rax + 8]",
        "call exo_tlsdesc_dynamic_helper",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "add rsp, 8",
        "ret",
    );
}

#[cfg(not(all(target_arch = "x86_64", not(test))))]
pub unsafe extern "C" fn exo_tlsdesc_dynamic() -> isize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant2_tls_slot_preserves_alignment_and_negative_offset() {
        let (c1, tp1) = compute_variant2_slot(0, 24, 16, 0).unwrap();
        assert_eq!(c1, 32);
        assert_eq!(tp1, -32);
        assert_eq!((-tp1) % 16, 0);

        let (c2, tp2) = compute_variant2_slot(c1, 8, 8, 0).unwrap();
        assert_eq!(c2, 40);
        assert_eq!(tp2, -40);
        assert_eq!((-tp2) % 8, 0);

        // Non-zero p_vaddr mod p_align check: p_vaddr % 16 == 8 with memsz = 8 -> cursor = 40 + 8 = 48 (-48) -> (-48) mod 16?
        // Wait: (0 - 48) mod 16 = 0, vs (0 - 56) mod 16 = 8 == vaddr_mod_align!
        let (c3, tp3) = compute_variant2_slot(c2, 8, 16, 8).unwrap();
        assert_eq!(c3, 56);
        assert_eq!(tp3, -56);
        assert_eq!((tp3.rem_euclid(16)), 8);
    }
}
