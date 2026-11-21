//! Dynamic linker runtime entry (`_dlstart_stage2`) and public C ABI (`dlopen`, `dlsym`, `dlclose`, `dlerror`, `dladdr`, `dl_iterate_phdr`, `_r_debug`).

pub mod library;
pub mod resolver;
pub mod search_path;
pub mod symbol_table;
pub mod version;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::alloc_mmap::SpinLock;
use crate::dynamic_linker::library::{
    extract_init_task, run_init_tasks, LinkerState, LoadedObject, LINKER,
};
use crate::dynamic_linker::resolver::{
    close_for_dlclose, load_dependencies_bfs, lookup_in_object, open_for_dlopen,
    parse_in_memory_phdrs, relocate_unrelocated_objects, resolve_in_state, run_fini_tasks,
    topological_init_order, RTLD_DEFAULT_ADDR, RTLD_NEXT_ADDR,
};
use crate::dynamic_linker::symbol_table::cstr_bytes;
use crate::elf::dyn_info::{DynamicInfo, Elf64Dyn};
use crate::elf::dso_map::{compute_relro_bounds, protect_relro, ParsedPhdr};
use crate::elf::parser::{Elf64Phdr, PT_DYNAMIC, PT_PHDR, PT_TLS};
use crate::elf::reloc_apply::{apply_rela_table, Elf64Rela, RelocTargetInfo};
use crate::elf::tls::{
    current_tcb_ptr, finalize_initial_thread_tls, register_tls_module, __exo_dl_alloc_thread_tls,
    __exo_dl_free_thread_tls, __tls_get_addr,
};

pub const AT_NULL: usize = 0;
pub const AT_PHDR: usize = 3;
pub const AT_PHENT: usize = 4;
pub const AT_PHNUM: usize = 5;
pub const AT_PAGESZ: usize = 6;
pub const AT_BASE: usize = 7;
pub const AT_ENTRY: usize = 9;
pub const AT_SECURE: usize = 23;
pub const AT_EXECFN: usize = 31;

/// Returned in `RAX:RDX` from `_dlstart_c` so `_start` switches to `main` with SysV stack intact.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserJump {
    pub entry: u64,
    pub rsp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DlInfo {
    pub dli_fname: *const c_char,
    pub dli_fbase: *mut c_void,
    pub dli_sname: *const c_char,
    pub dli_saddr: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DlPhdrInfo {
    pub dlpi_addr: usize,
    pub dlpi_name: *const c_char,
    pub dlpi_phdr: *const Elf64Phdr,
    pub dlpi_phnum: u16,
    pub dlpi_adds: u64,
    pub dlpi_subs: u64,
    pub dlpi_tls_modid: usize,
    pub dlpi_tls_data: *mut c_void,
}

#[repr(C)]
pub struct RDebug {
    pub r_version: i32,
    pub r_map: *mut c_void,
    pub r_brk: usize,
    pub r_state: i32,
    pub r_ldbase: usize,
}

#[no_mangle]
pub extern "C" fn _dl_debug_state() {}

#[no_mangle]
pub static mut _r_debug: RDebug = RDebug {
    r_version: 1,
    r_map: ptr::null_mut(),
    r_brk: 0,
    r_state: 0,
    r_ldbase: 0,
};

struct DlErrorState {
    has_err: bool,
    buf: [u8; 256],
}

impl DlErrorState {
    const fn new() -> Self {
        Self {
            has_err: false,
            buf: [0u8; 256],
        }
    }
}

static DL_ERROR: SpinLock<DlErrorState> = SpinLock::new(DlErrorState::new());
static ABI_ANCHOR_INIT: AtomicBool = AtomicBool::new(false);

fn set_dlerror(msg: &str) {
    let mut err = DL_ERROR.lock();
    err.has_err = true;
    err.buf.fill(0);
    let n = msg.len().min(err.buf.len() - 1);
    err.buf[..n].copy_from_slice(&msg.as_bytes()[..n]);
}

unsafe fn raw_cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    let bytes = unsafe { cstr_bytes(p as *const u8, 0, 4096) };
    core::str::from_utf8(bytes).ok()
}

fn find_env_ld_library_path(envp: *const *const u8) -> Option<String> {
    if envp.is_null() {
        return None;
    }
    let mut cur = envp;
    for _ in 0..4096 {
        let entry = unsafe { *cur };
        if entry.is_null() {
            break;
        }
        let s = unsafe { cstr_bytes(entry, 0, 8192) };
        if let Some(val) = s.strip_prefix(b"LD_LIBRARY_PATH=") {
            if let Ok(st) = core::str::from_utf8(val) {
                return Some(String::from(st));
            }
        }
        cur = unsafe { cur.add(1) };
    }
    None
}

fn extract_pt_info(phdrs: &[ParsedPhdr], phdr_vaddr: usize) -> (usize, usize, Option<ParsedPhdr>) {
    let mut pt_phdr_raw = 0u64;
    let mut pt_dyn_raw = 0u64;
    let mut tls = None;
    for ph in phdrs {
        match ph.p_type {
            PT_PHDR => pt_phdr_raw = ph.vaddr,
            PT_DYNAMIC => pt_dyn_raw = ph.vaddr,
            PT_TLS if ph.memsz > 0 => tls = Some(*ph),
            _ => {}
        }
    }
    let load_bias = if pt_phdr_raw != 0 {
        phdr_vaddr.wrapping_sub(pt_phdr_raw as usize)
    } else if let Some(first_load) = phdrs.iter().find(|p| p.p_type == 1) {
        // Fallback when PT_PHDR is omitted: e_phoff sits right after EHDR in first PT_LOAD
        if phdr_vaddr >= (first_load.vaddr as usize).wrapping_add(0x40) {
            phdr_vaddr.wrapping_sub(0x40).wrapping_sub(first_load.vaddr as usize)
        } else {
            0
        }
    } else {
        0
    };
    let dynamic_vaddr = if pt_dyn_raw != 0 {
        load_bias.wrapping_add(pt_dyn_raw as usize)
    } else {
        0
    };
    (load_bias, dynamic_vaddr, tls)
}

/// Stage 2 bootstrap executed after `_dlstart_c` finishes relative self-relocation of `ld-exo.so`.
/// Loads all `DT_NEEDED` libraries recursively, applies relocations + RELRO, builds combined TLS/DTV,
/// runs dependency constructors, and returns main executable entry + original SysV `%rsp`.
pub unsafe fn dlstart_stage2(
    sp: *const usize,
    ld_base: usize,
    ld_dynv: *const Elf64Dyn,
) -> Result<UserJump, i32> {
    ABI_ANCHOR_INIT.store(true, Ordering::Relaxed);

    if sp.is_null() {
        return Err(22);
    }
    let argc = unsafe { *sp };
    let mut cur = unsafe { sp.add(1 + argc + 1) };
    let envp = cur as *const *const u8;
    while unsafe { *cur } != 0 {
        cur = unsafe { cur.add(1) };
    }
    let mut auxv = unsafe { cur.add(1) };

    let mut at_phdr = 0usize;
    let mut at_phnum = 0u16;
    let mut at_entry = 0usize;
    let mut at_base = 0usize;
    let mut at_secure = 0usize;
    let mut at_execfn = ptr::null::<u8>();

    loop {
        let a_type = unsafe { *auxv };
        let a_val = unsafe { *auxv.add(1) };
        if a_type == AT_NULL {
            break;
        }
        match a_type {
            AT_PHDR => at_phdr = a_val,
            AT_PHNUM => at_phnum = a_val as u16,
            AT_ENTRY => at_entry = a_val,
            AT_BASE => at_base = a_val,
            AT_SECURE => at_secure = a_val,
            AT_EXECFN => at_execfn = a_val as *const u8,
            _ => {}
        }
        auxv = unsafe { auxv.add(2) };
    }

    let eff_ld_base = if at_base != 0 { at_base } else { ld_base };
    let ld_dyn_info = unsafe { DynamicInfo::from_dyn_ptr(ld_dynv as usize, eff_ld_base)? };

    // Finish IRELATIVE + PT_GNU_RELRO on ld-exo.so itself.
    let ld_phdrs = unsafe {
        let ehdr_phoff = core::ptr::read_unaligned((eff_ld_base + 32) as *const u64) as usize;
        let ehdr_phnum = core::ptr::read_unaligned((eff_ld_base + 56) as *const u16);
        parse_in_memory_phdrs(eff_ld_base.wrapping_add(ehdr_phoff), ehdr_phnum)
    };
    let ld_self_target = RelocTargetInfo {
        obj_idx: 1,
        load_bias: eff_ld_base,
        symtab: ld_dyn_info.symtab as *const _,
        strtab: ld_dyn_info.strtab as *const u8,
        strsz: ld_dyn_info.strsz,
        own_tls_modid: 0,
        own_tls_static_tpoff: 0,
        own_tls_is_static: false,
    };
    unsafe {
        // Re-pass with IRELATIVE support now that ld_base GOT works
        apply_rela_table(
            &ld_self_target,
            ld_dyn_info.rela as *const Elf64Rela,
            ld_dyn_info.relasz,
            ld_dyn_info.relaent,
            |_, _, _| None,
        )?;
        let _ = _r_debug;
        _r_debug.r_ldbase = eff_ld_base;
        _r_debug.r_brk = _dl_debug_state as *const () as usize;
    }
    let (ld_relro_start, ld_relro_len) = compute_relro_bounds(&ld_phdrs, eff_ld_base);
    protect_relro(ld_relro_start, ld_relro_len)?;

    let exec_path_str = if !at_execfn.is_null() {
        core::str::from_utf8(unsafe { cstr_bytes(at_execfn, 0, 4096) }).unwrap_or("/bin/app")
    } else {
        "/bin/app"
    };
    let main_phdrs = unsafe { parse_in_memory_phdrs(at_phdr, at_phnum) };
    let (main_bias, main_dyn_vaddr, main_tls) = extract_pt_info(&main_phdrs, at_phdr);
    let main_dyn_info = unsafe { DynamicInfo::from_dyn_ptr(main_dyn_vaddr, main_bias)? };
    let (main_span_base, main_span_len) =
        LinkerState::compute_kernel_mapped_span(&main_phdrs, main_bias);
    let (main_relro_start, main_relro_len) = compute_relro_bounds(&main_phdrs, main_bias);

    let (main_tls_modid, main_tls_tpoff, main_tls_static) = if let Some(tls) = main_tls {
        let init_image = main_bias.wrapping_add(tls.vaddr as usize);
        let (mid, tpoff) = register_tls_module(
            init_image,
            tls.filesz as usize,
            tls.memsz as usize,
            tls.align as usize,
            tls.vaddr as usize,
            true,
        )?;
        (mid, tpoff, tpoff != 0)
    } else {
        (0, 0, false)
    };

    let init_tasks = {
        let mut st = LINKER.lock();
        st.secure = at_secure != 0;
        if !st.secure {
            st.ld_library_path = find_env_ld_library_path(envp);
        }

        let mut main_path_cstr = Vec::with_capacity(exec_path_str.len() + 1);
        main_path_cstr.extend_from_slice(exec_path_str.as_bytes());
        main_path_cstr.push(0);

        let main_id = st.alloc_slot(LoadedObject {
            id: 0,
            path: String::from(exec_path_str),
            path_cstr: main_path_cstr,
            soname: main_dyn_info
                .soname_off
                .and_then(|o| unsafe { main_dyn_info.str_at(o) })
                .map(String::from),
            span_base: main_span_base,
            span_len: main_span_len,
            load_bias: main_bias,
            entry_vaddr: at_entry,
            phdr_vaddr: at_phdr,
            phnum: at_phnum,
            phdrs_owned: unsafe {
                if at_phdr != 0 && at_phnum > 0 {
                    core::slice::from_raw_parts(at_phdr as *const Elf64Phdr, at_phnum as usize)
                        .to_vec()
                } else {
                    Vec::new()
                }
            },
            dynamic_vaddr: main_dyn_vaddr,
            dyn_info: main_dyn_info,
            relro_start: main_relro_start,
            relro_len: main_relro_len,
            tls_modid: main_tls_modid,
            tls_static_tpoff: main_tls_tpoff,
            tls_is_static: main_tls_static,
            refcount: 1,
            global_scope: true,
            nodelete: true,
            is_root: true,
            is_interpreter: false,
            relocated: false,
            init_done: false,
            fini_done: false,
            direct_deps: Vec::new(),
        });

        let (ld_span_base, ld_span_len) =
            LinkerState::compute_kernel_mapped_span(&ld_phdrs, eff_ld_base);
        let ld_path = "/lib/ld-exo.so";
        let mut ld_cstr = Vec::from(b"/lib/ld-exo.so\0".as_slice());
        ld_cstr.shrink_to_fit();
        let ehdr_phoff = unsafe { ptr::read_unaligned((eff_ld_base + 32) as *const u64) as usize };
        let ehdr_phnum = unsafe { ptr::read_unaligned((eff_ld_base + 56) as *const u16) };
        let _ld_id = st.alloc_slot(LoadedObject {
            id: 1,
            path: String::from(ld_path),
            path_cstr: ld_cstr,
            soname: Some(String::from("ld-exo.so")),
            span_base: ld_span_base,
            span_len: ld_span_len,
            load_bias: eff_ld_base,
            entry_vaddr: eff_ld_base,
            phdr_vaddr: eff_ld_base.wrapping_add(ehdr_phoff),
            phnum: ehdr_phnum,
            phdrs_owned: unsafe {
                if ehdr_phnum > 0 {
                    core::slice::from_raw_parts(
                        eff_ld_base.wrapping_add(ehdr_phoff) as *const Elf64Phdr,
                        ehdr_phnum as usize,
                    )
                    .to_vec()
                } else {
                    Vec::new()
                }
            },
            dynamic_vaddr: ld_dynv as usize,
            dyn_info: ld_dyn_info,
            relro_start: ld_relro_start,
            relro_len: ld_relro_len,
            tls_modid: 0,
            tls_static_tpoff: 0,
            tls_is_static: false,
            refcount: 1,
            global_scope: true,
            nodelete: true,
            is_root: false,
            is_interpreter: true,
            relocated: true,
            init_done: true,
            fini_done: true,
            direct_deps: Vec::new(),
        });

        let _ = load_dependencies_bfs(&mut st, &[main_id], true, false)?;

        // Seal static TLS blocks and build combined %fs:0 + DTV before any IRELATIVE/constructors.
        drop(st);
        unsafe { finalize_initial_thread_tls()? };

        let mut st2 = LINKER.lock();
        relocate_unrelocated_objects(&mut st2, true)?;
        let order = topological_init_order(&st2, &[main_id]);
        let mut tasks = Vec::new();
        for id in order {
            if id == main_id {
                // Preinit runs for root, while normal init_array runs in CRT unless DSO.
                if let Some(obj) = st2.get_mut(id) {
                    if let Some(mut task) = unsafe { extract_init_task(obj) } {
                        task.init_fn = 0;
                        task.init_array_fns.clear();
                        if !task.preinit_fns.is_empty() {
                            tasks.push(task);
                        }
                    }
                }
            } else if let Some(obj) = st2.get_mut(id) {
                if let Some(task) = unsafe { extract_init_task(obj) } {
                    tasks.push(task);
                }
            }
        }
        tasks
    };

    unsafe { run_init_tasks(&init_tasks) };
    Ok(UserJump {
        entry: at_entry as u64,
        rsp: sp as u64,
    })
}

fn encode_handle(obj_id: usize) -> *mut c_void {
    (obj_id + 1) as *mut c_void
}

fn decode_handle(handle: *mut c_void) -> Option<usize> {
    let val = handle as usize;
    if val == 0 {
        None
    } else {
        Some(val - 1)
    }
}

#[no_mangle]
pub unsafe extern "C" fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void {
    let req = unsafe { raw_cstr(filename) };
    if !filename.is_null() && req.is_none() {
        set_dlerror("dlopen: invalid UTF-8 filename");
        return ptr::null_mut();
    }
    match open_for_dlopen(req, flags) {
        Ok((id, tasks)) => {
            unsafe { run_init_tasks(&tasks) };
            encode_handle(id)
        }
        Err(_) => {
            set_dlerror("dlopen: failed to load shared object");
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn dlsym_impl(
    handle: *mut c_void,
    symbol: *const c_char,
    caller_pc: usize,
) -> *mut c_void {
    let Some(sym_str) = (unsafe { raw_cstr(symbol) }) else {
        set_dlerror("dlsym: invalid symbol name");
        return ptr::null_mut();
    };
    let name_bytes = sym_str.as_bytes();
    let h_val = handle as usize;
    let st = LINKER.lock();

    let hit = if h_val == RTLD_DEFAULT_ADDR {
        resolve_in_state(&st, name_bytes, false, None, true)
    } else if h_val == RTLD_NEXT_ADDR {
        let caller_idx = st.find_by_pc(caller_pc).unwrap_or(0);
        let pos_in_order = st
            .load_order
            .iter()
            .position(|&id| id == caller_idx)
            .unwrap_or(0);
        let mut found = None;
        for &id in &st.load_order[pos_in_order + 1..] {
            if let Some(obj) = st.get(id) {
                if !obj.global_scope {
                    continue;
                }
                if let Some((_, sym)) = unsafe { lookup_in_object(obj, name_bytes) } {
                    found = Some(crate::elf::reloc_apply::ResolvedSymbol {
                        addr: obj.load_bias.wrapping_add(sym.st_value as usize),
                        size: sym.st_size as usize,
                        st_value: sym.st_value,
                        obj_base: obj.load_bias,
                        tls_modid: obj.tls_modid,
                        tls_static_tpoff: obj.tls_static_tpoff,
                        tls_is_static: obj.tls_is_static,
                    });
                    break;
                }
            }
        }
        found
    } else {
        let Some(id) = decode_handle(handle) else {
            set_dlerror("dlsym: invalid handle");
            return ptr::null_mut();
        };
        if st.get(id).is_none() {
            set_dlerror("dlsym: closed handle");
            return ptr::null_mut();
        }
        let order = topological_init_order(&st, &[id]);
        let mut found = None;
        for dep_id in order.into_iter().rev() {
            if let Some(obj) = st.get(dep_id) {
                if let Some((_, sym)) = unsafe { lookup_in_object(obj, name_bytes) } {
                    found = Some(crate::elf::reloc_apply::ResolvedSymbol {
                        addr: obj.load_bias.wrapping_add(sym.st_value as usize),
                        size: sym.st_size as usize,
                        st_value: sym.st_value,
                        obj_base: obj.load_bias,
                        tls_modid: obj.tls_modid,
                        tls_static_tpoff: obj.tls_static_tpoff,
                        tls_is_static: obj.tls_is_static,
                    });
                    break;
                }
            }
        }
        found
    };

    match hit {
        Some(res) => res.addr as *mut c_void,
        None => {
            set_dlerror("dlsym: undefined symbol");
            ptr::null_mut()
        }
    }
}

#[cfg(all(target_arch = "x86_64", not(test)))]
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void {
    core::arch::naked_asm!("mov rdx, [rsp]", "jmp dlsym_impl");
}

#[cfg(not(all(target_arch = "x86_64", not(test))))]
#[no_mangle]
pub unsafe extern "C" fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void {
    unsafe { dlsym_impl(handle, symbol, 0) }
}

#[no_mangle]
pub unsafe extern "C" fn dlclose(handle: *mut c_void) -> c_int {
    let Some(id) = decode_handle(handle) else {
        set_dlerror("dlclose: null handle");
        return -1;
    };
    match close_for_dlclose(id) {
        Ok(tasks) => {
            unsafe { run_fini_tasks(&tasks) };
            0
        }
        Err(_) => {
            set_dlerror("dlclose: invalid handle");
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn dlerror() -> *mut c_char {
    let mut err = DL_ERROR.lock();
    if !err.has_err {
        return ptr::null_mut();
    }
    err.has_err = false;
    err.buf.as_mut_ptr() as *mut c_char
}

#[no_mangle]
pub unsafe extern "C" fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int {
    if addr.is_null() || info.is_null() {
        return 0;
    }
    let pc = addr as usize;
    let st = LINKER.lock();
    let Some(id) = st.find_by_pc(pc) else {
        return 0;
    };
    let Some(obj) = st.get(id) else {
        return 0;
    };
    unsafe {
        (*info).dli_fname = obj.path_cstr.as_ptr() as *const c_char;
        (*info).dli_fbase = obj.load_bias as *mut c_void;
        (*info).dli_sname = ptr::null();
        (*info).dli_saddr = ptr::null_mut();
    }
    1
}

#[no_mangle]
pub unsafe extern "C" fn dl_iterate_phdr(
    callback: Option<unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int>,
    data: *mut c_void,
) -> c_int {
    let Some(cb) = callback else {
        return 0;
    };

    struct OwnedEntry {
        bias: usize,
        name: Vec<u8>,
        phdrs: Vec<Elf64Phdr>,
        adds: u64,
        subs: u64,
        tls_modid: usize,
        tls_data: *mut c_void,
    }

    let tcb = unsafe { current_tcb_ptr() };
    let dtv = if tcb.is_null() {
        ptr::null()
    } else {
        unsafe { (*tcb).dtv }
    };
    let dtv_cnt = if dtv.is_null() { 0 } else { unsafe { *dtv } };

    let snapshot: Vec<OwnedEntry> = {
        let st = LINKER.lock();
        let adds = st.adds_count;
        let subs = st.subs_count;
        let mut out = Vec::with_capacity(st.load_order.len());
        for &id in &st.load_order {
            if let Some(obj) = st.get(id) {
                let tls_data = if obj.tls_modid > 0 && obj.tls_modid <= dtv_cnt {
                    unsafe { *dtv.add(obj.tls_modid) as *mut c_void }
                } else {
                    ptr::null_mut()
                };
                out.push(OwnedEntry {
                    bias: obj.load_bias,
                    name: obj.path_cstr.clone(),
                    phdrs: obj.phdrs_owned.clone(),
                    adds,
                    subs,
                    tls_modid: obj.tls_modid,
                    tls_data,
                });
            }
        }
        out
    };

    for item in snapshot {
        let mut info = DlPhdrInfo {
            dlpi_addr: item.bias,
            dlpi_name: if item.name.is_empty() {
                b"\0".as_ptr() as *const c_char
            } else {
                item.name.as_ptr() as *const c_char
            },
            dlpi_phdr: item.phdrs.as_ptr(),
            dlpi_phnum: item.phdrs.len() as u16,
            dlpi_adds: item.adds,
            dlpi_subs: item.subs,
            dlpi_tls_modid: item.tls_modid,
            dlpi_tls_data: item.tls_data,
        };
        let ret = unsafe { cb(&mut info, core::mem::size_of::<DlPhdrInfo>(), data) };
        if ret != 0 {
            return ret;
        }
    }
    0
}

/// Anchors public dynamic ABI exports against aggressive LTO/GC elimination.
pub fn abi_export_table() -> [usize; 11] {
    [
        dlopen as *const () as usize,
        dlsym as *const () as usize,
        dlclose as *const () as usize,
        dlerror as *const () as usize,
        dladdr as *const () as usize,
        dl_iterate_phdr as *const () as usize,
        __tls_get_addr as *const () as usize,
        __exo_dl_alloc_thread_tls as *const () as usize,
        __exo_dl_free_thread_tls as *const () as usize,
        _dl_debug_state as *const () as usize,
        ptr::addr_of!(_r_debug) as usize,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_mmap::MmapLoaderAlloc;

    #[test]
    fn dlerror_sets_and_resets_after_read() {
        set_dlerror("test failure");
        let p1 = unsafe { dlerror() };
        assert!(!p1.is_null());
        let p2 = unsafe { dlerror() };
        assert!(p2.is_null());
    }

    #[test]
    fn encode_decode_handle_and_abi_exports_are_non_null() {
        let h = encode_handle(0);
        assert_eq!(decode_handle(h), Some(0));
        assert_eq!(decode_handle(ptr::null_mut()), None);
        let tbl = abi_export_table();
        assert!(tbl.iter().all(|&p| p != 0));
        let _ = MmapLoaderAlloc::new();
    }
}
