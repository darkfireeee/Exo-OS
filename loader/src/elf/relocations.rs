pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE: u32 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationError {
    Unsupported,
    OutOfRange,
    MissingSymbol,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct RelaEntry {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}

impl RelaEntry {
    #[inline]
    pub const fn reloc_type(self) -> u32 {
        self.info as u32
    }

    #[inline]
    pub const fn symbol_index(self) -> u32 {
        (self.info >> 32) as u32
    }
}

pub trait SymbolResolver {
    fn resolve(&self, symbol_index: u32) -> Option<u64>;
}

pub struct NoSymbols;

impl SymbolResolver for NoSymbols {
    fn resolve(&self, _symbol_index: u32) -> Option<u64> {
        None
    }
}

/// Applique une table RELA déjà mappée dans l'espace utilisateur courant.
///
/// `load_base` est ajouté aux offsets de relocation ET aux addends RELATIVE,
/// conformément au modèle ELF ET_DYN x86_64.
///
/// # Safety
/// `rela_vaddr..rela_vaddr + rela_count * sizeof(RelaEntry)` et toutes les
/// cibles de relocation doivent être mappées en écriture dans l'image courante.
pub unsafe fn apply_rela_table<R: SymbolResolver>(
    load_base: u64,
    rela_vaddr: u64,
    rela_count: usize,
    resolver: &R,
) -> Result<(), RelocationError> {
    let entries = core::slice::from_raw_parts(rela_vaddr as *const RelaEntry, rela_count);
    let mut idx = 0usize;
    while idx < entries.len() {
        apply_rela(load_base, entries[idx], resolver)?;
        idx += 1;
    }
    Ok(())
}

unsafe fn apply_rela<R: SymbolResolver>(
    load_base: u64,
    rela: RelaEntry,
    resolver: &R,
) -> Result<(), RelocationError> {
    let target = load_base
        .checked_add(rela.offset)
        .ok_or(RelocationError::OutOfRange)? as *mut u64;
    match rela.reloc_type() {
        R_X86_64_NONE => Ok(()),
        R_X86_64_RELATIVE => {
            let value = load_base.wrapping_add(rela.addend as u64);
            core::ptr::write_unaligned(target, value);
            Ok(())
        }
        R_X86_64_64 | R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
            let sym = if rela.symbol_index() == 0 {
                0
            } else {
                resolver
                    .resolve(rela.symbol_index())
                    .ok_or(RelocationError::MissingSymbol)?
            };
            let value = sym.wrapping_add(rela.addend as u64);
            core::ptr::write_unaligned(target, value);
            Ok(())
        }
        _ => Err(RelocationError::Unsupported),
    }
}
