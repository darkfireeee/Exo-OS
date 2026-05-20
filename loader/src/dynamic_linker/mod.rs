pub mod library;
pub mod resolver;
pub mod search_path;
pub mod symbol_table;
pub mod version;

use crate::elf::dynamic::{parse_dynamic_table, DynamicError, DynamicInfo, DT_RELA};
use crate::elf::relocations::{apply_rela_table, NoSymbols, RelaEntry, RelocationError};

pub const DYNAMIC_LOADER_HANDOFF_MAGIC: u64 = 0x5845_4f4c_4459_4e01;
pub const DYNAMIC_LOADER_HANDOFF_VERSION: u32 = 1;
pub const DYNAMIC_LOADER_PATH_MAX: usize = 128;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DynamicLoaderHandoff {
    pub magic: u64,
    pub version: u32,
    pub flags: u32,
    pub executable_base: u64,
    pub executable_entry: u64,
    pub executable_phdr: u64,
    pub executable_phnum: u64,
    pub executable_phent: u64,
    pub executable_dynamic: u64,
    pub executable_dynamic_count: u64,
    pub interpreter_base: u64,
    pub interpreter_entry: u64,
    pub page_size: u64,
    pub executable_path_len: u32,
    pub interpreter_path_len: u32,
    pub executable_path: [u8; DYNAMIC_LOADER_PATH_MAX],
    pub interpreter_path: [u8; DYNAMIC_LOADER_PATH_MAX],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserJump {
    pub entry: u64,
    pub arg0: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoaderError {
    NullHandoff,
    BadMagic,
    UnsupportedVersion,
    EmptyEntry,
    Dynamic(DynamicError),
    Relocation(RelocationError),
    UnsupportedNeededLibrary,
    UnsupportedPltRelocation,
    BadRelocationEntrySize,
}

impl From<DynamicError> for LoaderError {
    fn from(value: DynamicError) -> Self {
        Self::Dynamic(value)
    }
}

impl From<RelocationError> for LoaderError {
    fn from(value: RelocationError) -> Self {
        Self::Relocation(value)
    }
}

/// Point d'entrée logique du chargeur dynamique Exo-OS.
///
/// Le kernel a déjà mappé l'exécutable principal et l'interpréteur. Cette
/// routine finalise l'image principale: lecture de `PT_DYNAMIC`, application
/// des RELA supportées, appels d'initialisation locaux, puis retour de l'entrée
/// utilisateur à appeler.
///
/// # Safety
/// `handoff` doit être un pointeur utilisateur valide vers le contrat ABI posé
/// par le kernel sur la pile du processus.
pub unsafe fn runtime_entry(handoff: *const DynamicLoaderHandoff) -> Result<UserJump, LoaderError> {
    if handoff.is_null() {
        return Err(LoaderError::NullHandoff);
    }
    let handoff = &*handoff;
    validate_handoff(handoff)?;

    let dynamic = parse_dynamic_table(
        handoff.executable_dynamic,
        handoff.executable_dynamic_count as usize,
    )?;
    relocate_executable(handoff.executable_base, &dynamic)?;
    run_initializers(handoff.executable_base, &dynamic);

    Ok(UserJump {
        entry: handoff.executable_entry,
        arg0: 0,
    })
}

fn validate_handoff(handoff: &DynamicLoaderHandoff) -> Result<(), LoaderError> {
    if handoff.magic != DYNAMIC_LOADER_HANDOFF_MAGIC {
        return Err(LoaderError::BadMagic);
    }
    if handoff.version != DYNAMIC_LOADER_HANDOFF_VERSION {
        return Err(LoaderError::UnsupportedVersion);
    }
    if handoff.executable_entry == 0 {
        return Err(LoaderError::EmptyEntry);
    }
    Ok(())
}

unsafe fn relocate_executable(load_base: u64, dynamic: &DynamicInfo) -> Result<(), LoaderError> {
    if dynamic.needed_count != 0 {
        return Err(LoaderError::UnsupportedNeededLibrary);
    }

    let resolver = NoSymbols;
    if dynamic.has_rela() {
        let entry_size = if dynamic.rela_entry_size == 0 {
            core::mem::size_of::<RelaEntry>() as u64
        } else {
            dynamic.rela_entry_size
        };
        if entry_size != core::mem::size_of::<RelaEntry>() as u64 {
            return Err(LoaderError::BadRelocationEntrySize);
        }
        apply_rela_table(
            load_base,
            load_base.wrapping_add(dynamic.rela),
            (dynamic.rela_size / entry_size) as usize,
            &resolver,
        )?;
    }

    if dynamic.has_jmprel() {
        if dynamic.plt_rel_type != DT_RELA {
            return Err(LoaderError::UnsupportedPltRelocation);
        }
        apply_rela_table(
            load_base,
            load_base.wrapping_add(dynamic.jmprel),
            (dynamic.pltrel_size / core::mem::size_of::<RelaEntry>() as u64) as usize,
            &resolver,
        )?;
    }
    Ok(())
}

unsafe fn run_initializers(load_base: u64, dynamic: &DynamicInfo) {
    if dynamic.init != 0 {
        let init: extern "C" fn() = core::mem::transmute((load_base + dynamic.init) as usize);
        init();
    }

    if dynamic.init_array != 0 && dynamic.init_array_size != 0 {
        let count = (dynamic.init_array_size / core::mem::size_of::<u64>() as u64) as usize;
        let entries = core::slice::from_raw_parts(
            load_base.wrapping_add(dynamic.init_array) as *const u64,
            count,
        );
        let mut idx = 0usize;
        while idx < entries.len() {
            let func_addr = entries[idx];
            if func_addr != 0 {
                let init: extern "C" fn() = core::mem::transmute(func_addr as usize);
                init();
            }
            idx += 1;
        }
    }
}
