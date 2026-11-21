//! Dynamic dependency BFS loader, topological constructor ordering, symbol resolution, and relocation driver.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::dynamic_linker::library::{
    extract_fini_task, extract_init_task, FiniTask, InitTask, LinkerState, LoadedObject, LINKER,
};
use crate::dynamic_linker::search_path::{dirname_of, open_library_file};
use crate::dynamic_linker::symbol_table::{
    lookup_gnu_hash, lookup_sysv_hash, Elf64Sym, STB_WEAK,
};
use crate::elf::dyn_info::{DynamicInfo, DF_STATIC_TLS};
use crate::elf::dso_map::{map_elf_from_fd, protect_relro, ParsedPhdr};
use crate::elf::parser::Elf64Phdr;
use crate::elf::reloc_apply::{apply_rela_table, Elf64Rela, RelocTargetInfo, ResolvedSymbol};
use crate::elf::tls::register_tls_module;
use crate::syscall::{sys_close, sys_munmap};

pub const RTLD_LAZY: i32 = 1;
pub const RTLD_NOW: i32 = 2;
pub const RTLD_NOLOAD: i32 = 4;
pub const RTLD_GLOBAL: i32 = 0x100;
pub const RTLD_LOCAL: i32 = 0;
pub const RTLD_NODELETE: i32 = 0x1000;

pub const RTLD_DEFAULT_ADDR: usize = 0;
pub const RTLD_NEXT_ADDR: usize = usize::MAX;

/// Looks up `sym_name` inside a single `LoadedObject` (`DT_GNU_HASH` preferred, fallback `DT_HASH`).
pub unsafe fn lookup_in_object(obj: &LoadedObject, sym_name: &[u8]) -> Option<(u32, Elf64Sym)> {
    let symtab = obj.dyn_info.symtab as *const Elf64Sym;
    let strtab = obj.dyn_info.strtab as *const u8;
    let strsz = obj.dyn_info.strsz;
    if symtab.is_null() || strtab.is_null() {
        return None;
    }
    if obj.dyn_info.gnu_hash != 0 {
        return unsafe {
            lookup_gnu_hash(
                sym_name,
                obj.dyn_info.gnu_hash as *const u32,
                symtab,
                strtab,
                strsz,
            )
        };
    }
    if obj.dyn_info.hash != 0 {
        return unsafe {
            lookup_sysv_hash(
                sym_name,
                obj.dyn_info.hash as *const u32,
                symtab,
                strtab,
                strsz,
            )
        };
    }
    None
}

fn to_resolved_symbol(obj: &LoadedObject, sym: Elf64Sym) -> ResolvedSymbol {
    ResolvedSymbol {
        addr: if sym.st_shndx == 0xfff1 {
            sym.st_value as usize
        } else {
            obj.load_bias.wrapping_add(sym.st_value as usize)
        },
        size: sym.st_size as usize,
        st_value: sym.st_value,
        obj_base: obj.load_bias,
        tls_modid: obj.tls_modid,
        tls_static_tpoff: obj.tls_static_tpoff,
        tls_is_static: obj.tls_is_static,
    }
}

/// Resolves `sym_name` using global ELF scope (`DT_SYMBOLIC` local preference, strong before weak).
pub fn resolve_in_state(
    state: &LinkerState,
    sym_name: &[u8],
    skip_root: bool,
    requester_id: Option<usize>,
    global_only: bool,
) -> Option<ResolvedSymbol> {
    let mut first_weak: Option<ResolvedSymbol> = None;

    let try_obj = |obj: &LoadedObject, weak_slot: &mut Option<ResolvedSymbol>| -> Option<ResolvedSymbol> {
        if let Some((_, sym)) = unsafe { lookup_in_object(obj, sym_name) } {
            let res = to_resolved_symbol(obj, sym);
            if sym.bind() != STB_WEAK {
                return Some(res);
            }
            if weak_slot.is_none() {
                *weak_slot = Some(res);
            }
        }
        None
    };

    if let Some(req_id) = requester_id {
        if let Some(req) = state.get(req_id) {
            if req.dyn_info.has_symbolic && !(skip_root && req.is_root) {
                if let Some(hit) = try_obj(req, &mut first_weak) {
                    return Some(hit);
                }
            }
        }
    }

    for &id in &state.load_order {
        let Some(obj) = state.get(id) else {
            continue;
        };
        if skip_root && obj.is_root {
            continue;
        }
        if global_only && !obj.global_scope && Some(id) != requester_id {
            continue;
        }
        if let Some(hit) = try_obj(obj, &mut first_weak) {
            return Some(hit);
        }
    }
    first_weak
}

/// Computes dependency-first topological order (`deps` before `dependents`) without duplicates.
pub fn topological_init_order(
    state: &LinkerState,
    roots: &[usize],
) -> Vec<usize> {
    let mut visited = Vec::new();
    let mut order = Vec::new();
    fn visit(
        state: &LinkerState,
        id: usize,
        visited: &mut Vec<usize>,
        order: &mut Vec<usize>,
    ) {
        if visited.contains(&id) {
            return;
        }
        visited.push(id);
        if let Some(obj) = state.get(id) {
            for &dep_id in &obj.direct_deps {
                visit(state, dep_id, visited, order);
            }
        }
        order.push(id);
    }
    for &r in roots {
        visit(state, r, &mut visited, &mut order);
    }
    order
}

#[inline]
fn to_raw_phdrs(phdrs: &[ParsedPhdr]) -> Vec<Elf64Phdr> {
    phdrs
        .iter()
        .map(|ph| Elf64Phdr {
            p_type: ph.p_type,
            p_flags: ph.flags,
            p_offset: ph.offset,
            p_vaddr: ph.vaddr,
            p_paddr: ph.vaddr,
            p_filesz: ph.filesz,
            p_memsz: ph.memsz,
            p_align: ph.align,
        })
        .collect()
}

/// Parses `ParsedPhdr` from raw `Elf64Phdr` slice in memory (main executable & interpreter).
pub unsafe fn parse_in_memory_phdrs(phdr_vaddr: usize, phnum: u16) -> Vec<ParsedPhdr> {
    let mut out = Vec::with_capacity(phnum as usize);
    if phdr_vaddr == 0 || phnum == 0 {
        return out;
    }
    let slice = unsafe { core::slice::from_raw_parts(phdr_vaddr as *const Elf64Phdr, phnum as usize) };
    for ph in slice {
        out.push(ParsedPhdr {
            p_type: ph.p_type,
            flags: ph.p_flags,
            offset: ph.p_offset,
            vaddr: ph.p_vaddr,
            filesz: ph.p_filesz,
            memsz: ph.p_memsz,
            align: ph.p_align,
        });
    }
    out
}

/// Loads `needed_names` via BFS starting from `(seed_id)` and returns all newly added object IDs.
pub fn load_dependencies_bfs(
    state: &mut LinkerState,
    seed_ids: &[usize],
    global_scope: bool,
    nodelete: bool,
) -> Result<Vec<usize>, i32> {
    let mut queue: VecDeque<usize> = VecDeque::new();
    for &id in seed_ids {
        queue.push_back(id);
    }
    let mut newly_added = Vec::new();

    while let Some(parent_id) = queue.pop_front() {
        let (needed, parent_origin, parent_rpath, parent_runpath, llp, secure) = {
            let Some(parent) = state.get(parent_id) else {
                continue;
            };
            let needed_strs: Vec<String> = unsafe { parent.dyn_info.needed_names() }
                .into_iter()
                .map(String::from)
                .collect();
            (
                needed_strs,
                String::from(dirname_of(&parent.path)),
                parent.rpath_str().map(String::from),
                parent.runpath_str().map(String::from),
                state.ld_library_path.clone(),
                state.secure,
            )
        };

        let mut resolved_deps = Vec::with_capacity(needed.len());
        for dep_name in needed {
            // Canonical alias: `ld-exo.so` or `libc.so` aliases resolve consistently
            if let Some(existing_id) = state.find_by_name(&dep_name) {
                if let Some(obj) = state.get_mut(existing_id) {
                    obj.refcount = obj.refcount.saturating_add(1);
                    if global_scope {
                        obj.global_scope = true;
                    }
                    if nodelete {
                        obj.nodelete = true;
                    }
                }
                resolved_deps.push(existing_id);
                continue;
            }

            let Some((fd, resolved_path)) = open_library_file(
                &dep_name,
                &parent_origin,
                parent_rpath.as_deref(),
                parent_runpath.as_deref(),
                llp.as_deref(),
                secure,
            ) else {
                return Err(2); // ENOENT
            };

            let mapped = match map_elf_from_fd(fd) {
                Ok(m) => {
                    unsafe {
                        let _ = sys_close(fd);
                    }
                    m
                }
                Err(e) => {
                    unsafe {
                        let _ = sys_close(fd);
                    }
                    return Err(e);
                }
            };

            let dyn_info = unsafe { DynamicInfo::from_dyn_ptr(mapped.dynamic_vaddr, mapped.load_bias)? };
            let soname = dyn_info
                .soname_off
                .and_then(|off| unsafe { dyn_info.str_at(off) })
                .map(String::from);

            // Re-check canonical path or SONAME against already loaded objects
            if let Some(dup_id) = state
                .find_by_name(&resolved_path)
                .or_else(|| soname.as_deref().and_then(|s| state.find_by_name(s)))
            {
                unsafe {
                    let _ = sys_munmap(mapped.span_base, mapped.span_len);
                }
                if let Some(obj) = state.get_mut(dup_id) {
                    obj.refcount = obj.refcount.saturating_add(1);
                    if global_scope {
                        obj.global_scope = true;
                    }
                }
                resolved_deps.push(dup_id);
                continue;
            }

            let (tls_modid, tls_static_tpoff, tls_is_static) = if let Some(tls) = mapped.tls {
                let init_image = mapped.load_bias.wrapping_add(tls.vaddr as usize);
                let static_req = (dyn_info.flags & DF_STATIC_TLS) != 0;
                let (mid, tpoff) = register_tls_module(
                    init_image,
                    tls.filesz as usize,
                    tls.memsz as usize,
                    tls.align as usize,
                    tls.vaddr as usize,
                    static_req,
                )?;
                (mid, tpoff, tpoff != 0)
            } else {
                (0, 0, false)
            };

            let mut path_cstr = Vec::with_capacity(resolved_path.len() + 1);
            path_cstr.extend_from_slice(resolved_path.as_bytes());
            path_cstr.push(0);

            let owned_phdrs = to_raw_phdrs(&mapped.phdrs_storage);
            let obj = LoadedObject {
                id: 0,
                path: resolved_path,
                path_cstr,
                soname,
                span_base: mapped.span_base,
                span_len: mapped.span_len,
                load_bias: mapped.load_bias,
                entry_vaddr: mapped.entry_vaddr,
                phdr_vaddr: mapped.phdr_vaddr,
                phnum: mapped.phnum,
                phdrs_owned: owned_phdrs,
                dynamic_vaddr: mapped.dynamic_vaddr,
                dyn_info,
                relro_start: mapped.relro_start,
                relro_len: mapped.relro_len,
                tls_modid,
                tls_static_tpoff,
                tls_is_static,
                refcount: 1,
                global_scope,
                nodelete,
                is_root: false,
                is_interpreter: false,
                relocated: false,
                init_done: false,
                fini_done: false,
                direct_deps: Vec::new(),
            };
            let new_id = state.alloc_slot(obj);
            newly_added.push(new_id);
            resolved_deps.push(new_id);
            queue.push_back(new_id);
        }

        if let Some(parent_mut) = state.get_mut(parent_id) {
            parent_mut.direct_deps = resolved_deps;
        }
    }

    Ok(newly_added)
}

/// Applies relocations (`DT_RELA` + `DT_JMPREL`) and `PT_GNU_RELRO` for every unrelocated object.
pub fn relocate_unrelocated_objects(
    state: &mut LinkerState,
    skip_interpreter: bool,
) -> Result<(), i32> {
    let ids: Vec<usize> = state
        .load_order
        .iter()
        .copied()
        .filter(|&id| {
            state
                .get(id)
                .map(|o| !o.relocated && !(skip_interpreter && o.is_interpreter))
                .unwrap_or(false)
        })
        .collect();

    // Dependencies before dependents (ensures copy reloc sources / RELRO bases are ready).
    for id in ids.into_iter().rev() {
        let Some(obj) = state.get(id) else {
            continue;
        };
        let target = RelocTargetInfo {
            obj_idx: id,
            load_bias: obj.load_bias,
            symtab: obj.dyn_info.symtab as *const Elf64Sym,
            strtab: obj.dyn_info.strtab as *const u8,
            strsz: obj.dyn_info.strsz,
            own_tls_modid: obj.tls_modid,
            own_tls_static_tpoff: obj.tls_static_tpoff,
            own_tls_is_static: obj.tls_is_static,
        };
        let rela_ptr = obj.dyn_info.rela as *const Elf64Rela;
        let relasz = obj.dyn_info.relasz;
        let relaent = obj.dyn_info.relaent;
        let jmprel_ptr = obj.dyn_info.jmprel as *const Elf64Rela;
        let pltrelsz = obj.dyn_info.pltrelsz;
        let relro_start = obj.relro_start;
        let relro_len = obj.relro_len;

        unsafe {
            apply_rela_table(&target, rela_ptr, relasz, relaent, |name, skip_root, req| {
                resolve_in_state(state, name, skip_root, Some(req), false)
            })?;
            apply_rela_table(
                &target,
                jmprel_ptr,
                pltrelsz,
                core::mem::size_of::<Elf64Rela>(),
                |name, skip_root, req| {
                    resolve_in_state(state, name, skip_root, Some(req), false)
                },
            )?;
        }
        protect_relro(relro_start, relro_len)?;
        if let Some(obj_mut) = state.get_mut(id) {
            obj_mut.relocated = true;
        }
    }
    Ok(())
}

/// Opens a shared library (or main handle if `path == None`) for `dlopen`.
pub fn open_for_dlopen(path: Option<&str>, flags: i32) -> Result<(usize, Vec<InitTask>), i32> {
    let global_scope = (flags & RTLD_GLOBAL) != 0;
    let nodelete = (flags & RTLD_NODELETE) != 0;
    let noload = (flags & RTLD_NOLOAD) != 0;

    let mut st = LINKER.lock();
    let target_id = if let Some(req_name) = path {
        if let Some(existing) = st.find_by_name(req_name) {
            if let Some(obj) = st.get_mut(existing) {
                if !noload {
                    obj.refcount = obj.refcount.saturating_add(1);
                }
                if global_scope {
                    obj.global_scope = true;
                }
                if nodelete {
                    obj.nodelete = true;
                }
            }
            existing
        } else {
            if noload {
                return Err(2); // ENOENT
            }
            let llp = st.ld_library_path.clone();
            let secure = st.secure;
            let Some((fd, resolved_path)) = open_library_file(
                req_name,
                "/lib",
                None,
                None,
                llp.as_deref(),
                secure,
            ) else {
                return Err(2);
            };
            let mapped = match map_elf_from_fd(fd) {
                Ok(m) => {
                    unsafe {
                        let _ = sys_close(fd);
                    }
                    m
                }
                Err(e) => {
                    unsafe {
                        let _ = sys_close(fd);
                    }
                    return Err(e);
                }
            };
            let dyn_info = unsafe { DynamicInfo::from_dyn_ptr(mapped.dynamic_vaddr, mapped.load_bias)? };
            let soname = dyn_info
                .soname_off
                .and_then(|off| unsafe { dyn_info.str_at(off) })
                .map(String::from);

            if let Some(dup_id) = st
                .find_by_name(&resolved_path)
                .or_else(|| soname.as_deref().and_then(|s| st.find_by_name(s)))
            {
                unsafe {
                    let _ = sys_munmap(mapped.span_base, mapped.span_len);
                }
                if let Some(obj) = st.get_mut(dup_id) {
                    if !noload {
                        obj.refcount = obj.refcount.saturating_add(1);
                    }
                    if global_scope {
                        obj.global_scope = true;
                    }
                    if nodelete {
                        obj.nodelete = true;
                    }
                }
                dup_id
            } else {
                let (tls_modid, tls_static_tpoff, tls_is_static) = if let Some(tls) = mapped.tls {
                    let init_image = mapped.load_bias.wrapping_add(tls.vaddr as usize);
                    let static_req = (dyn_info.flags & DF_STATIC_TLS) != 0;
                    let (mid, tpoff) = register_tls_module(
                        init_image,
                        tls.filesz as usize,
                        tls.memsz as usize,
                        tls.align as usize,
                        tls.vaddr as usize,
                        static_req,
                    )?;
                    (mid, tpoff, tpoff != 0)
                } else {
                    (0, 0, false)
                };

                let mut path_cstr = Vec::with_capacity(resolved_path.len() + 1);
                path_cstr.extend_from_slice(resolved_path.as_bytes());
                path_cstr.push(0);
                let owned_phdrs = to_raw_phdrs(&mapped.phdrs_storage);
                let id = st.alloc_slot(LoadedObject {
                    id: 0,
                    path: resolved_path,
                    path_cstr,
                    soname,
                    span_base: mapped.span_base,
                    span_len: mapped.span_len,
                    load_bias: mapped.load_bias,
                    entry_vaddr: mapped.entry_vaddr,
                    phdr_vaddr: mapped.phdr_vaddr,
                    phnum: mapped.phnum,
                    phdrs_owned: owned_phdrs,
                    dynamic_vaddr: mapped.dynamic_vaddr,
                    dyn_info,
                    relro_start: mapped.relro_start,
                    relro_len: mapped.relro_len,
                    tls_modid,
                    tls_static_tpoff,
                    tls_is_static,
                    refcount: 1,
                    global_scope,
                    nodelete,
                    is_root: false,
                    is_interpreter: false,
                    relocated: false,
                    init_done: false,
                    fini_done: false,
                    direct_deps: Vec::new(),
                });
                let _ = load_dependencies_bfs(&mut st, &[id], global_scope, nodelete)?;
                relocate_unrelocated_objects(&mut st, true)?;
                id
            }
        }
    } else {
        0 // Main executable handle (slot 0)
    };

    let init_ids = topological_init_order(&st, &[target_id]);
    let mut tasks = Vec::new();
    for id in init_ids {
        if let Some(obj) = st.get_mut(id) {
            if let Some(task) = unsafe { extract_init_task(obj) } {
                tasks.push(task);
            }
        }
    }
    Ok((target_id, tasks))
}

/// Decrements refcount and collects fini/unmap tasks when `refcount == 0` and closeable.
pub fn close_for_dlclose(handle_id: usize) -> Result<Vec<FiniTask>, i32> {
    let mut st = LINKER.lock();
    let Some(obj) = st.get_mut(handle_id) else {
        return Err(22); // EINVAL
    };
    if obj.is_root || obj.is_interpreter || obj.nodelete {
        return Ok(Vec::new());
    }
    if obj.refcount > 1 {
        obj.refcount -= 1;
        return Ok(Vec::new());
    }
    obj.refcount = 0;
    let task = unsafe { extract_fini_task(obj, true) };
    st.objects[handle_id] = None;
    st.load_order.retain(|&id| id != handle_id);
    st.subs_count = st.subs_count.wrapping_add(1);
    Ok(alloc::vec![task])
}

pub unsafe fn run_fini_tasks(tasks: &[FiniTask]) {
    for t in tasks {
        for &fn_addr in &t.fini_array_fns {
            let f: extern "C" fn() = unsafe { core::mem::transmute(fn_addr) };
            f();
        }
        if t.fini_fn != 0 {
            let f: extern "C" fn() = unsafe { core::mem::transmute(t.fini_fn) };
            f();
        }
        if let Some((base, len)) = t.unmap_span {
            unsafe {
                let _ = sys_munmap(base, len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn dummy_obj(id: usize, deps: Vec<usize>) -> LoadedObject {
        LoadedObject {
            id,
            path: alloc::format!("/lib/lib{}.so", id),
            path_cstr: Vec::new(),
            soname: None,
            span_base: 0x1000 * (id + 1),
            span_len: 0x1000,
            load_bias: 0,
            entry_vaddr: 0,
            phdr_vaddr: 0,
            phnum: 0,
            phdrs_owned: Vec::new(),
            dynamic_vaddr: 0,
            dyn_info: DynamicInfo::default(),
            relro_start: 0,
            relro_len: 0,
            tls_modid: 0,
            tls_static_tpoff: 0,
            tls_is_static: false,
            refcount: 1,
            global_scope: true,
            nodelete: false,
            is_root: id == 0,
            is_interpreter: false,
            relocated: true,
            init_done: false,
            fini_done: false,
            direct_deps: deps,
        }
    }

    #[test]
    fn topological_init_order_emits_leaves_before_roots() {
        let mut st = LinkerState::new();
        let id0 = st.alloc_slot(dummy_obj(0, vec![1, 2]));
        let id1 = st.alloc_slot(dummy_obj(1, vec![3]));
        let id2 = st.alloc_slot(dummy_obj(2, vec![3]));
        let id3 = st.alloc_slot(dummy_obj(3, vec![]));
        assert_eq!((id0, id1, id2, id3), (0, 1, 2, 3));

        let order = topological_init_order(&st, &[0]);
        assert_eq!(order, vec![3, 1, 2, 0]);
    }
}
