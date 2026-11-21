//! Loaded shared object registry, global scope, reference counts, and init/fini snapshots.

use alloc::string::String;
use alloc::vec::Vec;

use crate::alloc_mmap::SpinLock;
use crate::elf::dyn_info::DynamicInfo;
use crate::elf::dso_map::ParsedPhdr;
use crate::elf::parser::Elf64Phdr;

#[derive(Clone, Debug)]
pub struct LoadedObject {
    pub id: usize,
    pub path: String,
    pub path_cstr: Vec<u8>,
    pub soname: Option<String>,
    pub span_base: usize,
    pub span_len: usize,
    pub load_bias: usize,
    pub entry_vaddr: usize,
    pub phdr_vaddr: usize,
    pub phnum: u16,
    /// Keeps kernel/synthetic program headers alive for `dl_iterate_phdr` when not in mapped ELF.
    pub phdrs_owned: Vec<Elf64Phdr>,
    pub dynamic_vaddr: usize,
    pub dyn_info: DynamicInfo,
    pub relro_start: usize,
    pub relro_len: usize,
    pub tls_modid: usize,
    pub tls_static_tpoff: isize,
    pub tls_is_static: bool,
    pub refcount: usize,
    pub global_scope: bool,
    pub nodelete: bool,
    pub is_root: bool,
    pub is_interpreter: bool,
    pub relocated: bool,
    pub init_done: bool,
    pub fini_done: bool,
    pub direct_deps: Vec<usize>,
}

impl LoadedObject {
    pub fn matches_name(&self, query: &str) -> bool {
        if self.path == query {
            return true;
        }
        if let Some(ref so) = self.soname {
            if so == query {
                return true;
            }
        }
        if !query.contains('/') {
            if let Some(base) = self.path.rsplit('/').next() {
                if base == query {
                    return true;
                }
            }
        }
        false
    }

    pub fn contains_pc(&self, pc: usize) -> bool {
        self.span_len > 0 && pc >= self.span_base && pc < self.span_base.saturating_add(self.span_len)
    }

    pub fn rpath_str(&self) -> Option<&str> {
        self.dyn_info
            .rpath_off
            .and_then(|off| unsafe { self.dyn_info.str_at(off) })
    }

    pub fn runpath_str(&self) -> Option<&str> {
        self.dyn_info
            .runpath_off
            .and_then(|off| unsafe { self.dyn_info.str_at(off) })
    }
}

#[derive(Clone, Debug)]
pub struct InitTask {
    pub obj_id: usize,
    pub preinit_fns: Vec<usize>,
    pub init_fn: usize,
    pub init_array_fns: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct FiniTask {
    pub fini_array_fns: Vec<usize>,
    pub fini_fn: usize,
    pub unmap_span: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct PhdrSnapshotItem {
    pub addr: usize,
    pub name_ptr: *const u8,
    pub phdr_ptr: *const Elf64Phdr,
    pub phnum: u16,
    pub adds: u64,
    pub subs: u64,
    pub tls_modid: usize,
    pub tls_data: *mut u8,
}

pub struct LinkerState {
    pub objects: Vec<Option<LoadedObject>>,
    pub load_order: Vec<usize>,
    pub ld_library_path: Option<String>,
    pub secure: bool,
    pub adds_count: u64,
    pub subs_count: u64,
}

impl LinkerState {
    pub const fn new() -> Self {
        Self {
            objects: Vec::new(),
            load_order: Vec::new(),
            ld_library_path: None,
            secure: false,
            adds_count: 0,
            subs_count: 0,
        }
    }

    pub fn get(&self, id: usize) -> Option<&LoadedObject> {
        self.objects.get(id).and_then(|slot| slot.as_ref())
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut LoadedObject> {
        self.objects.get_mut(id).and_then(|slot| slot.as_mut())
    }

    pub fn alloc_slot(&mut self, mut obj: LoadedObject) -> usize {
        if let Some(pos) = self.objects.iter().position(|s| s.is_none()) {
            obj.id = pos;
            self.objects[pos] = Some(obj);
            self.load_order.push(pos);
            self.adds_count = self.adds_count.wrapping_add(1);
            return pos;
        }
        let id = self.objects.len();
        obj.id = id;
        self.objects.push(Some(obj));
        self.load_order.push(id);
        self.adds_count = self.adds_count.wrapping_add(1);
        id
    }

    pub fn find_by_name(&self, name: &str) -> Option<usize> {
        for &id in &self.load_order {
            if let Some(obj) = self.get(id) {
                if obj.matches_name(name) {
                    return Some(id);
                }
            }
        }
        None
    }

    pub fn find_by_pc(&self, pc: usize) -> Option<usize> {
        for &id in &self.load_order {
            if let Some(obj) = self.get(id) {
                if obj.contains_pc(pc) {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Computes ELF span from owned `ParsedPhdr` list for objects already mapped by kernel.
    pub fn compute_kernel_mapped_span(phdrs: &[ParsedPhdr], load_bias: usize) -> (usize, usize) {
        let mut min_v = usize::MAX;
        let mut max_v = 0usize;
        for ph in phdrs {
            if ph.p_type == crate::elf::parser::PT_LOAD && ph.memsz > 0 {
                let start = load_bias.wrapping_add(ph.vaddr as usize) & !0xFFF;
                let end = (load_bias.wrapping_add(ph.vaddr as usize) + ph.memsz as usize + 0xFFF)
                    & !0xFFF;
                if start < min_v {
                    min_v = start;
                }
                if end > max_v {
                    max_v = end;
                }
            }
        }
        if min_v == usize::MAX {
            (0, 0)
        } else {
            (min_v, max_v.saturating_sub(min_v))
        }
    }
}

pub static LINKER: SpinLock<LinkerState> = SpinLock::new(LinkerState::new());

/// Extracts function pointer lists (`preinit`, `init`, `init_array`) cleanly outside of locks.
pub unsafe fn extract_init_task(obj: &mut LoadedObject) -> Option<InitTask> {
    if obj.init_done {
        return None;
    }
    obj.init_done = true;

    let read_fn_array = |base: usize, byte_len: usize| -> Vec<usize> {
        let mut out = Vec::new();
        if base == 0 || byte_len == 0 {
            return out;
        }
        let cnt = byte_len / core::mem::size_of::<usize>();
        let ptr = base as *const usize;
        for i in 0..cnt {
            let addr = unsafe { *ptr.add(i) };
            if addr != 0 && addr != usize::MAX {
                out.push(addr);
            }
        }
        out
    };

    let preinit_fns = if obj.is_root {
        read_fn_array(obj.dyn_info.preinit_array, obj.dyn_info.preinit_arraysz)
    } else {
        Vec::new()
    };
    let init_array_fns = read_fn_array(obj.dyn_info.init_array, obj.dyn_info.init_arraysz);
    Some(InitTask {
        obj_id: obj.id,
        preinit_fns,
        init_fn: obj.dyn_info.init,
        init_array_fns,
    })
}

/// Runs extracted constructors (`DT_PREINIT_ARRAY`, `DT_INIT`, `DT_INIT_ARRAY`) without holding locks.
pub unsafe fn run_init_tasks(tasks: &[InitTask]) {
    for t in tasks {
        for &fn_addr in &t.preinit_fns {
            let f: extern "C" fn() = unsafe { core::mem::transmute(fn_addr) };
            f();
        }
        if t.init_fn != 0 {
            let f: extern "C" fn() = unsafe { core::mem::transmute(t.init_fn) };
            f();
        }
        for &fn_addr in &t.init_array_fns {
            let f: extern "C" fn() = unsafe { core::mem::transmute(fn_addr) };
            f();
        }
    }
}

/// Extracts reversed `DT_FINI_ARRAY` + `DT_FINI` cleanly before unmapping.
pub unsafe fn extract_fini_task(obj: &mut LoadedObject, unmap: bool) -> FiniTask {
    let mut fini_array_fns = Vec::new();
    let mut fini_fn = 0usize;
    if !obj.fini_done {
        obj.fini_done = true;
        if obj.dyn_info.fini_array != 0 && obj.dyn_info.fini_arraysz != 0 {
            let cnt = obj.dyn_info.fini_arraysz / core::mem::size_of::<usize>();
            let ptr = obj.dyn_info.fini_array as *const usize;
            let mut i = cnt;
            while i > 0 {
                i -= 1;
                let addr = unsafe { *ptr.add(i) };
                if addr != 0 && addr != usize::MAX {
                    fini_array_fns.push(addr);
                }
            }
        }
        fini_fn = obj.dyn_info.fini;
    }
    let unmap_span = if unmap && !obj.is_root && !obj.is_interpreter && obj.span_len > 0 {
        Some((obj.span_base, obj.span_len))
    } else {
        None
    };
    FiniTask {
        fini_array_fns,
        fini_fn,
        unmap_span,
    }
}
