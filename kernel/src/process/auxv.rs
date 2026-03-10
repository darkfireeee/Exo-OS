//! process/auxv.rs — Vecteur auxiliaire (auxv) pour do_exec()
//!
//! ## Référence spec : ExoFS_Syscall_Analyse.md §8 – BUG-04 / Exécution TLS
//!
//! Le vecteur auxv est une liste de paires (type: u64, valeur: u64) placée
//! sur la pile de l'espace utilisateur lors de `execve()`, après argv et envp.
//!
//! ## Constantes AT_*
//! Toutes les constantes AT_* Linux standard plus les extensions Exo-OS.
//!
//! ## Fonction push_auxv
//! Construit le vecteur auxv sur la pile userspace et retourne le nouveau
//! pointeur de pile après écriture.
//!
//! ## Règle BUG-04
//! AT_SIGNAL_TCB (51) doit être inclus pour que exo-rt puisse localiser le
//! SignalTcb sans adresse fixe (SIG-18 : jamais d'adresse fixe, ASLR).


use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes AT_* standard Linux
// ─────────────────────────────────────────────────────────────────────────────

/// Fin du vecteur auxv.
pub const AT_NULL:         u64 =  0;
/// Ignoré.
pub const AT_IGNORE:       u64 =  1;
/// Descripteur de fichier du fichier exécutable.
pub const AT_EXECFD:       u64 =  2;
/// Adresse des en-têtes de programme ELF.
pub const AT_PHDR:         u64 =  3;
/// Taille d'une entrée d'en-tête de programme.
pub const AT_PHENT:        u64 =  4;
/// Nombre d'en-têtes de programme.
pub const AT_PHNUM:        u64 =  5;
/// Taille d'une page système.
pub const AT_PAGESZ:       u64 =  6;
/// Adresse de base de l'interpréteur ELF.
pub const AT_BASE:         u64 =  7;
/// Flags du programme.
pub const AT_FLAGS:        u64 =  8;
/// Point d'entrée du programme.
pub const AT_ENTRY:        u64 =  9;
/// Non-zero si le programme n'est PAR l'ELF interpréteur.
pub const AT_NOTELF:       u64 = 10;
/// UID réel.
pub const AT_UID:          u64 = 11;
/// UID effectif.
pub const AT_EUID:         u64 = 12;
/// GID réel.
pub const AT_GID:          u64 = 13;
/// GID effectif.
pub const AT_EGID:         u64 = 14;
/// Chaîne identifiant la plateforme.
pub const AT_PLATFORM:     u64 = 15;
/// Bitmask HWCAP des capacités hardware.
pub const AT_HWCAP:        u64 = 16;
/// Fréquence des ticks de l'horloge.
pub const AT_CLKTCK:       u64 = 17;
/// Adresse virtuelle du fichier vDSO.
pub const AT_SYSINFO:      u64 = 32;
/// Adresse de la page ELF du vDSO (pour fast-path clock_gettime).
pub const AT_SYSINFO_EHDR: u64 = 33;
/// Bitmask HWCAP2 étendu.
pub const AT_HWCAP2:       u64 = 26;
/// Pointeur vers 16 bytes de données aléatoires (cookie sécurité).
pub const AT_RANDOM:       u64 = 25;

// ─────────────────────────────────────────────────────────────────────────────
// Extensions Exo-OS
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse virtuelle du SignalTcb (SIG-18 — jamais fixe, ASLR).
/// Utilisé par exo-rt pour localiser les handlers de signaux.
pub const AT_SIGNAL_TCB: u64 = 51;

/// Token de capability initial du processus (Exo-OS security layer).
/// Passé par le noyau lors de exec() depuis init_server.
pub const AT_CAP_TOKEN:  u64 = 52;

// ─────────────────────────────────────────────────────────────────────────────
// Structure auxv
// ─────────────────────────────────────────────────────────────────────────────

/// Une paire (type, valeur) du vecteur auxiliaire.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct AuxEntry {
    pub a_type: u64,
    pub a_val:  u64,
}

impl AuxEntry {
    #[inline]
    pub const fn new(a_type: u64, a_val: u64) -> Self {
        Self { a_type, a_val }
    }
    /// Entrée de terminaison.
    #[inline]
    pub const fn null() -> Self {
        Self { a_type: AT_NULL, a_val: 0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Paramètres de construction du vecteur auxv
// ─────────────────────────────────────────────────────────────────────────────

/// Informations ELF nécessaires à la construction du vecteur auxv.
pub struct AuxvParams {
    /// Adresse des en-têtes de programme (PT_PHDR ou calculée depuis EHDR).
    pub phdr_vaddr:       u64,
    /// Nombre d'entrées de programme (e_phnum).
    pub phnum:            u64,
    /// Point d'entrée du programme (e_entry, après relocation pour PIE).
    pub entry_vaddr:      u64,
    /// Adresse de base de l'interpréteur (ld-linux, ld-exo).
    pub interp_base:      u64,
    /// Adresse de la page ELF vDSO (pour fast-path syscalls).
    pub vdso_ehdr_vaddr:  u64,
    /// Adresse du SignalTcb mappé en userspace (SIG-18).
    pub signal_tcb_vaddr: u64,
    /// Token de capability initial.
    pub cap_token:        u64,
    /// UID/GID du processus.
    pub uid:              u32,
    pub gid:              u32,
    /// Pointeur vers 16 bytes aléatoires DÉJÀ sur la pile.
    pub random_ptr:       u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Construction du vecteur auxv
// ─────────────────────────────────────────────────────────────────────────────

/// Construit la liste des entrées auxv à partir des paramètres d'exécution.
///
/// Retourne un Vec<AuxEntry> à sérialiser sur la pile userspace.
/// OOM-02 : try_reserve explicite.
pub fn build_auxv(params: &AuxvParams) -> Result<Vec<AuxEntry>, ()> {
    let mut v: Vec<AuxEntry> = Vec::new();
    // Nombre maximal connu : ~16 entrées + AT_NULL.
    v.try_reserve(20).map_err(|_| ())?;

    v.push(AuxEntry::new(AT_PHDR,         params.phdr_vaddr));
    v.push(AuxEntry::new(AT_PHENT,        64)); // Elf64_Phdr = 56 bytes, arrondi à 64
    v.push(AuxEntry::new(AT_PHNUM,        params.phnum));
    v.push(AuxEntry::new(AT_PAGESZ,       4096));
    v.push(AuxEntry::new(AT_BASE,         params.interp_base));
    v.push(AuxEntry::new(AT_FLAGS,        0));
    v.push(AuxEntry::new(AT_ENTRY,        params.entry_vaddr));
    v.push(AuxEntry::new(AT_UID,          params.uid as u64));
    v.push(AuxEntry::new(AT_EUID,         params.uid as u64));
    v.push(AuxEntry::new(AT_GID,          params.gid as u64));
    v.push(AuxEntry::new(AT_EGID,         params.gid as u64));
    v.push(AuxEntry::new(AT_RANDOM,       params.random_ptr));
    v.push(AuxEntry::new(AT_CLKTCK,       100));
    v.push(AuxEntry::new(AT_HWCAP,        0));

    // vDSO : présent seulement si l'adresse de la page est non nulle.
    if params.vdso_ehdr_vaddr != 0 {
        v.push(AuxEntry::new(AT_SYSINFO_EHDR, params.vdso_ehdr_vaddr));
    }

    // Extensions Exo-OS
    if params.signal_tcb_vaddr != 0 {
        v.push(AuxEntry::new(AT_SIGNAL_TCB, params.signal_tcb_vaddr));
    }
    if params.cap_token != 0 {
        v.push(AuxEntry::new(AT_CAP_TOKEN, params.cap_token));
    }

    // Terminaison obligatoire
    v.push(AuxEntry::null());
    Ok(v)
}

/// Sérialise le vecteur auxv en bytes (pour copy_to_user sur la pile).
///
/// RECUR-01 : while, pas de for.
pub fn serialize_auxv(entries: &[AuxEntry]) -> Result<Vec<u8>, ()> {
    let byte_count = entries.len().checked_mul(core::mem::size_of::<AuxEntry>()).ok_or(())?;
    let mut buf = Vec::new();
    buf.try_reserve(byte_count).map_err(|_| ())?;
    let mut i = 0usize;
    while i < entries.len() {
        let e = &entries[i];
        // SAFETY: AuxEntry est #[repr(C)] — layout binaire stable.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                e as *const AuxEntry as *const u8,
                core::mem::size_of::<AuxEntry>(),
            )
        };
        let mut j = 0usize;
        while j < bytes.len() {
            buf.push(bytes[j]);
            j = j.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    Ok(buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// push_auxv : point d'entrée pour do_exec()
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit le vecteur auxv sur la pile userspace et retourne le SP résultant.
///
/// `stack_top` : pointeur vers le sommet de la pile AVANT l'écriture auxv.
/// Retourne le nouveau SP (stack_top - taille_auxv), aligné sur 8 bytes.
///
/// ## Usage dans do_exec()
/// ```ignore
/// let new_sp = push_auxv(user_stack_top, &params)?;
/// // Puis écrire environ/argv au-dessus de new_sp.
/// ```
pub fn push_auxv(stack_top: u64, params: &AuxvParams) -> Result<u64, ()> {
    let entries = build_auxv(params)?;
    let bytes   = serialize_auxv(&entries)?;
    let size    = bytes.len() as u64;
    // Aligner vers le bas sur 8 bytes.
    let sp = (stack_top.saturating_sub(size)) & !7u64;
    // Écrire via copy_to_user (valide le pointeur avant l'écriture).
    match crate::syscall::validation::copy_to_user(
        sp as *mut u8,
        bytes.as_ptr(),
        bytes.len(),
    ) {
        Ok(_)  => Ok(sp),
        Err(_) => Err(()),
    }
}
