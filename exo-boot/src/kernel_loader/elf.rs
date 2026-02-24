//! elf.rs — Parseur ELF64 et chargeur de segments kernel.
//!
//! RÈGLE BOOT-07 (DOC10) :
//!   "Le kernel est compilé en PIE. Le bootloader charge les PT_LOAD à
//!    l'adresse physique randomisée kaslr_base + offset.
//!    Les relocations R_X86_64_RELATIVE sont appliquées ensuite."
//!
//! Ce module :
//!   1. Valide le header ELF64 (magic, architecture, type)
//!   2. Parse les Program Headers (PT_LOAD)
//!   3. Mappe les segments en mémoire physique allouée
//!   4. Expose l'entry point et la taille totale
//!
//! Référence : System V ABI AMD64 Supplement + ELF Specification 1.2

// ─── Constantes ELF ──────────────────────────────────────────────────────────

const ELF_MAGIC:    [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64:   u8      = 2;
const ELFDATA2LSB:  u8      = 1;    // Little-endian
const ET_EXEC:      u16     = 2;    // Executable
const ET_DYN:       u16     = 3;    // Shared object (PIE kernel)
const EM_X86_64:    u16     = 62;
const PT_LOAD:      u32     = 1;
const PT_DYNAMIC:   u32     = 2;
#[allow(dead_code)]
const PT_GNU_RELRO: u32     = 0x6474_E552;

// Flags de segment ELF
#[allow(dead_code)]
const PF_X: u32 = 1;    // Execute
#[allow(dead_code)]
const PF_W: u32 = 2;    // Write
#[allow(dead_code)]
const PF_R: u32 = 4;    // Read

// ─── Structures ELF64 ────────────────────────────────────────────────────────

/// Header ELF64.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Elf64Header {
    e_ident:        [u8; 16],
    e_type:         u16,
    e_machine:      u16,
    e_version:      u32,
    e_entry:        u64,    // Entry point (adresse virtuelle dans l'ELF)
    e_phoff:        u64,    // Offset du Program Header Table
    e_shoff:        u64,    // Offset du Section Header Table
    e_flags:        u32,
    e_ehsize:       u16,
    e_phentsize:    u16,    // Taille d'un Program Header Entry
    e_phnum:        u16,    // Nombre de Program Headers
    e_shentsize:    u16,
    e_shnum:        u16,
    e_shstrndx:     u16,
}

/// Program Header ELF64.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Elf64ProgramHeader {
    pub(crate) p_type:   u32,  // Type du segment (PT_*)
    pub(crate) p_flags:  u32,  // Flags (PF_R | PF_W | PF_X)
    pub(crate) p_offset: u64,  // Offset dans le fichier
    pub(crate) p_vaddr:  u64,  // Adresse virtuelle dans le processus
    pub(crate) p_paddr:  u64,  // Adresse physique (ignorée en userland, utilisée ici)
    pub(crate) p_filesz: u64,  // Taille dans le fichier
    pub(crate) p_memsz:  u64,  // Taille en mémoire (≥ filesz — reste à zéro)
    pub(crate) p_align:  u64,  // Alignement (puissance de 2)
}

// ─── Interface publique ──────────────────────────────────────────────────────

/// Kernel ELF parsé, prêt à être chargé.
pub struct ElfKernel<'a> {
    /// Données brutes de l'image ELF.
    data:           &'a [u8],
    /// Header ELF parsé.
    header:         Elf64Header,
    /// Adresse virtuelle minimale des segments PT_LOAD.
    virt_base:      u64,
    /// Adresse virtuelle maximale de fin des segments PT_LOAD.
    virt_end:       u64,
    /// Entry point virtuel (relatif à virt_base pour PIE).
    entry_virt:     u64,
    /// `true` si le kernel est PIE (ET_DYN).
    pub is_pie:     bool,
}

impl<'a> ElfKernel<'a> {
    /// Parse un buffer contenant une image ELF64.
    /// Retourne `Err` si le format est invalide ou non supporté.
    pub fn parse(data: &'a [u8]) -> Result<Self, ElfError> {
        if data.len() < core::mem::size_of::<Elf64Header>() {
            return Err(ElfError::TooSmall { size: data.len() });
        }

        // Lire le header de façon alignment-safe
        let header: Elf64Header = unsafe {
            core::ptr::read_unaligned(data.as_ptr() as *const Elf64Header)
        };

        // 1. Valide le magic
        if &header.e_ident[0..4] != &ELF_MAGIC {
            return Err(ElfError::InvalidMagic { got: [
                header.e_ident[0], header.e_ident[1],
                header.e_ident[2], header.e_ident[3],
            ]});
        }

        // 2. Vérifie class + endianness
        if header.e_ident[4] != ELFCLASS64 {
            return Err(ElfError::WrongClass { class: header.e_ident[4] });
        }
        if header.e_ident[5] != ELFDATA2LSB {
            return Err(ElfError::WrongEndianness);
        }

        // 3. Vérifie le type (exécutable ou PIE)
        let is_pie = match header.e_type {
            ET_EXEC => false,
            ET_DYN  => true,
            other   => return Err(ElfError::WrongType { got: other }),
        };

        // 4. Vérifie l'architecture
        if header.e_machine != EM_X86_64 {
            return Err(ElfError::WrongArch { machine: header.e_machine });
        }

        // 5. Vérifie la cohérence des offsets
        if header.e_phentsize as usize != core::mem::size_of::<Elf64ProgramHeader>() {
            return Err(ElfError::InvalidPhentsize { size: header.e_phentsize });
        }
        let ph_end = header.e_phoff as usize
            + header.e_phnum as usize * core::mem::size_of::<Elf64ProgramHeader>();
        if ph_end > data.len() {
            return Err(ElfError::ProgramHeaderOutOfBounds {
                ph_end,
                file_size: data.len(),
            });
        }

        // 6. Calcule les bornes virtuelles des segments PT_LOAD
        let (virt_base, virt_end) = compute_virt_bounds(&header, data)?;

        Ok(Self {
            data,
            entry_virt: header.e_entry,
            header,
            virt_base,
            virt_end,
            is_pie,
        })
    }

    /// Taille totale de l'image en mémoire (virt_end - virt_base), alignée page.
    pub fn load_size(&self) -> u64 {
        let raw_size = self.virt_end.saturating_sub(self.virt_base);
        super::super::memory::map::align_up(raw_size, crate::memory::PAGE_SIZE as u64)
    }

    /// Offset de l'entry point depuis virt_base.
    /// Pour un kernel PIE : kernel_load_addr + entry_offset = adresse d'exécution.
    pub fn entry_offset(&self) -> u64 {
        self.entry_virt.saturating_sub(self.virt_base)
    }

    /// Adresse physique préférée pour le chargement (segment 0 p_paddr).
    /// Pertinent uniquement pour les kernels non-PIE.
    pub fn preferred_load_address(&self) -> u64 {
        self.virt_base
    }

    /// Charge tous les segments PT_LOAD à `phys_load_base`.
    ///
    /// Le kernel est copié depuis les données ELF vers la mémoire physique.
    /// Les bytes `filesz..memsz` sont mis à zéro (BSS).
    ///
    /// `phys_load_base` : adresse physique cible (après KASLR pour PIE,
    ///                    ou adresse fixe pour exécutables).
    ///
    /// # Safety
    /// `phys_load_base` doit pointer vers de la mémoire physique accessible
    /// et de taille ≥ `load_size()`.
    pub unsafe fn load_segments(&self, phys_load_base: u64) -> Result<(), ElfError> {
        let phs = self.program_headers();

        for ph in phs {
            if ph.p_type != PT_LOAD {
                continue;
            }
            if ph.p_memsz == 0 {
                continue;
            }

            // Offset de ce segment par rapport à virt_base
            let seg_offset = ph.p_vaddr.saturating_sub(self.virt_base);
            let dest_phys  = phys_load_base + seg_offset;

            // Valide que le segment est dans le fichier ELF
            let src_end = ph.p_offset as usize + ph.p_filesz as usize;
            if src_end > self.data.len() {
                return Err(ElfError::SegmentOutOfFile {
                    offset: ph.p_offset,
                    size:   ph.p_filesz,
                    file:   self.data.len() as u64,
                });
            }

            // Copie les données du fichier vers la mémoire physique
            if ph.p_filesz > 0 {
                // SAFETY : src dans les bornes data (vérifié src_end <= data.len()),
                //          dst est la mémoire physique allouée par l'appelant.
                unsafe {
                    let src = self.data.as_ptr().add(ph.p_offset as usize);
                    let dst = dest_phys as *mut u8;
                    core::ptr::copy_nonoverlapping(src, dst, ph.p_filesz as usize);
                }
            }

            // Zéro-fill la partie BSS (memsz > filesz)
            if ph.p_memsz > ph.p_filesz {
                let bss_start = dest_phys + ph.p_filesz;
                let bss_size  = ph.p_memsz - ph.p_filesz;
                // SAFETY : bss_start est dans la mémoire allouée, après la partie copiée.
                unsafe {
                    core::ptr::write_bytes(bss_start as *mut u8, 0, bss_size as usize);
                }
            }
        }

        Ok(())
    }

    /// Retourne le slice des Program Headers.
    fn program_headers(&self) -> &[Elf64ProgramHeader] {
        let offset = self.header.e_phoff as usize;
        let count  = self.header.e_phnum as usize;
        // SAFETY : Validé dans parse() — dans les bounds du fichier.
        unsafe {
            core::slice::from_raw_parts(
                self.data.as_ptr().add(offset) as *const Elf64ProgramHeader,
                count,
            )
        }
    }

    /// Retourne le segment `.dynamic` (PT_DYNAMIC) si présent.
    /// Utilisé par `relocations.rs` pour trouver la table de relocations.
    pub(crate) fn dynamic_segment(&self) -> Option<Elf64ProgramHeader> {
        self.program_headers()
            .iter()
            .find(|ph| ph.p_type == PT_DYNAMIC)
            .copied()
    }

    /// Retourne les données brutes de l'image ELF.
    #[inline]
    pub fn raw_data(&self) -> &[u8] { self.data }

    /// Retourne virt_base (pour calcul des offsets de relocation).
    #[inline]
    pub fn virt_base(&self) -> u64 { self.virt_base }
}

// ─── Calcul bornes virtuelles ─────────────────────────────────────────────────

fn compute_virt_bounds(
    header: &Elf64Header,
    data:   &[u8],
) -> Result<(u64, u64), ElfError> {
    let mut virt_base = u64::MAX;
    let mut virt_end  = 0u64;

    let ph_ptr = unsafe {
        data.as_ptr().add(header.e_phoff as usize) as *const Elf64ProgramHeader
    };

    for i in 0..header.e_phnum as usize {
        let ph = unsafe { core::ptr::read_unaligned(ph_ptr.add(i)) };
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        virt_base = virt_base.min(ph.p_vaddr);
        let end   = ph.p_vaddr.checked_add(ph.p_memsz)
            .ok_or(ElfError::VirtAddressOverflow { vaddr: ph.p_vaddr, size: ph.p_memsz })?;
        virt_end  = virt_end.max(end);
    }

    if virt_base == u64::MAX {
        return Err(ElfError::NoPtLoadSegments);
    }

    Ok((virt_base, virt_end))
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ElfError {
    TooSmall              { size: usize },
    InvalidMagic          { got: [u8; 4] },
    WrongClass            { class: u8 },
    WrongEndianness,
    WrongType             { got: u16 },
    WrongArch             { machine: u16 },
    InvalidPhentsize      { size: u16 },
    ProgramHeaderOutOfBounds { ph_end: usize, file_size: usize },
    SegmentOutOfFile      { offset: u64, size: u64, file: u64 },
    NoPtLoadSegments,
    VirtAddressOverflow   { vaddr: u64, size: u64 },
}

impl core::fmt::Display for ElfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooSmall { size } =>
                write!(f, "Image trop petite : {} bytes", size),
            Self::InvalidMagic { got } =>
                write!(f, "Magic ELF invalide : {:02X}{:02X}{:02X}{:02X}",
                    got[0], got[1], got[2], got[3]),
            Self::WrongClass { class } =>
                write!(f, "Classe ELF incorrecte : {} (attendu 64-bit=2)", class),
            Self::WrongEndianness =>
                write!(f, "Endianness incorrect (big-endian non supporté)"),
            Self::WrongType { got } =>
                write!(f, "Type ELF incorrect : {} (attendu ET_EXEC=2 ou ET_DYN=3)", got),
            Self::WrongArch { machine } =>
                write!(f, "Architecture incorrecte : {} (attendu x86_64=62)", machine),
            Self::InvalidPhentsize { size } =>
                write!(f, "e_phentsize invalide : {} (attendu {})",
                    size, core::mem::size_of::<Elf64ProgramHeader>()),
            Self::ProgramHeaderOutOfBounds { ph_end, file_size } =>
                write!(f, "Program headers hors fichier : {} > {}", ph_end, file_size),
            Self::SegmentOutOfFile { offset, size, file } =>
                write!(f, "Segment hors fichier : +{}+{} > {}", offset, size, file),
            Self::NoPtLoadSegments =>
                write!(f, "Aucun segment PT_LOAD dans l'image ELF"),
            Self::VirtAddressOverflow { vaddr, size } =>
                write!(f, "Overflow d'adresse virtuelle : {:#x} + {:#x}", vaddr, size),
        }
    }
}
