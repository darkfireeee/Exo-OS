//! file.rs — EFI_FILE_PROTOCOL — lecture FAT32/ESP.
//!
//! Utilisé pour charger le kernel ELF et la configuration exo-boot.cfg
//! depuis la partition ESP (EFI System Partition, FAT32).
//!
//! Le chemin d'accès est relatif à la racine du volume EFI sur lequel
//! exo-boot lui-même a été chargé (obtenu via EFI_LOADED_IMAGE_PROTOCOL).
//!
//! Chemins par défaut (surchargeable via config) :
//!   - Kernel    : \EFI\EXOOS\kernel.elf
//!   - Config    : \EFI\EXOOS\exo-boot.cfg
//!   - Initrd    : \EFI\EXOOS\initrd.img (optionnel)

use uefi::prelude::*;
use uefi::proto::media::file::{
    File, FileAttribute, FileInfo, FileMode, FileType,
};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::loaded_image::LoadedImage;
use uefi::CStr16;

// ─── Constantes ───────────────────────────────────────────────────────────────

/// Taille maximale d'un fichier chargeable par exo-boot.
/// 64 MB — suffisant pour un kernel compressé, irréaliste pour un kernel non compressé
/// sur systèmes avec peu de RAM. À ajuster si besoin.
const MAX_FILE_SIZE_BYTES: u64 = 64 * 1024 * 1024;

/// Taille du buffer pour FileInfo (nom de fichier inclus).
const FILE_INFO_BUFFER_SIZE: usize = 512;

// ─── API publique ─────────────────────────────────────────────────────────────

/// Charge un fichier depuis l'ESP et retourne son contenu dans un vecteur de bytes.
///
/// `path` est un chemin au format UEFI (séparateurs `\`, ex: `\EFI\EXOOS\kernel.elf`).
///
/// La mémoire retournée est allouée dans le pool UEFI (LOADER_DATA).
/// Elle sera réclamée par ExitBootServices si non explicitement libérée.
///
/// RÈGLE BOOT-02 : Le contenu brut est retourné — la vérification de signature
/// est effectuée par l'appelant via `kernel_loader/verify.rs`.
pub fn load_file<'p>(
    bt:            &BootServices,
    image_handle:  Handle,
    path:          &'p str,
) -> Result<FileBuffer, FileError<'p>> {
    crate::uefi::exit::assert_boot_services_active("load_file");

    // ── Récupération du volume racine ─────────────────────────────────────────
    let root_dir = open_root_dir(bt, image_handle)?;

    // ── Conversion du chemin UTF-8 → UCS-2 ───────────────────────────────────
    let mut path_buf = [0u16; 512];
    let _path_u16_len = utf8_to_ucs2(path, &mut path_buf)
        .map_err(|_| FileError::PathTooLong { path, max_chars: 511 })?;

    // ── Ouverture du fichier ──────────────────────────────────────────────────
    let mut root = root_dir;
    // SAFETY : path_buf contient une chaîne UCS-2 null-terminée valide.
    // path_buf est maintenant utilisé directement dans le même scope.
    let file_handle = {
        let cpath = unsafe { CStr16::from_ptr(path_buf.as_ptr() as *const uefi::Char16) };
        root.open(cpath, FileMode::Read, FileAttribute::empty())
            .map_err(|e| FileError::NotFound {
                path,
                status: e.status(),
            })?
    };

    let mut regular_file = match file_handle.into_type()
        .map_err(|e| FileError::OpenFailed { path, status: e.status() })?
    {
        FileType::Regular(f) => f,
        FileType::Dir(_)     => return Err(FileError::IsDirectory { path }),
    };

    // ── Récupération de la taille du fichier ──────────────────────────────────
    let mut info_buf = [0u8; FILE_INFO_BUFFER_SIZE];
    let file_info: &FileInfo = regular_file
        .get_info::<FileInfo>(&mut info_buf)
        .map_err(|e| FileError::InfoFailed { path, status: e.status() })?;

    let file_size = file_info.file_size();

    if file_size == 0 {
        return Err(FileError::Empty { path });
    }
    if file_size > MAX_FILE_SIZE_BYTES {
        return Err(FileError::TooLarge {
            path,
            size:  file_size,
            limit: MAX_FILE_SIZE_BYTES,
        });
    }

    // ── Allocation du buffer de lecture ──────────────────────────────────────
    // On alloue via UEFI pool pour garantir l'alignement 8 bytes (ELF alignment).
    let buf_ptr = bt
        .allocate_pool(uefi::table::boot::MemoryType::LOADER_DATA, file_size as usize)
        .map_err(|e| FileError::AllocationFailed {
            size:   file_size as usize,
            status: e.status(),
        })?;

    // SAFETY : buf_ptr est valide, taille file_size, aligné UEFI.
    let buf = unsafe {
        core::slice::from_raw_parts_mut(buf_ptr, file_size as usize)
    };

    // ── Lecture du fichier ────────────────────────────────────────────────────
    let bytes_read = regular_file
        .read(buf)
        .map_err(|e| FileError::ReadFailed { path, status: e.status() })?;

    if bytes_read != file_size as usize {
        // Lecture partielle — libère le buffer et retourne une erreur
        // SAFETY : buf_ptr valide, alloué par allocate_pool.
        unsafe { let _ = bt.free_pool(buf_ptr); }
        return Err(FileError::PartialRead {
            path,
            expected: file_size as usize,
            got:      bytes_read,
        });
    }

    Ok(FileBuffer {
        ptr:  buf_ptr,
        size: bytes_read,
    })
}

/// Vérifie l'existence d'un fichier sur l'ESP sans le charger.
pub fn file_exists(
    bt:           &BootServices,
    image_handle: Handle,
    path:         &str,
) -> bool {
    let Ok(root) = open_root_dir(bt, image_handle) else { return false; };
    let mut path_buf = [0u16; 512];
    let Ok(_) = utf8_to_ucs2(path, &mut path_buf) else { return false; };
    let mut dir = root;
    // SAFETY : path_buf null-terminé valide, utilisé dans le même scope.
    let result = {
        let cpath = unsafe { CStr16::from_ptr(path_buf.as_ptr() as *const uefi::Char16) };
        dir.open(cpath, FileMode::Read, FileAttribute::empty()).is_ok()
    };
    result
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Ouvre le répertoire racine du volume depuis lequel exo-boot a été chargé.
fn open_root_dir(
    bt:           &BootServices,
    image_handle: Handle,
) -> Result<uefi::proto::media::file::Directory, FileError<'static>> {
    // ── Récupération du device handle via LoadedImage ─────────────────────────
    let loaded_image_scoped = bt
        .open_protocol_exclusive::<LoadedImage>(image_handle)
        .map_err(|_| FileError::LoadedImageProtocolNotFound)?;
    let loaded_image: &LoadedImage = &*loaded_image_scoped;
    let device_handle = loaded_image.device().ok_or(FileError::DeviceHandleNull)?;

    // ── Ouverture du SimpleFileSystem sur ce device ───────────────────────────
    let mut fs_scoped = bt
        .open_protocol_exclusive::<SimpleFileSystem>(device_handle)
        .map_err(|e| FileError::SimpleFileSystemNotFound { status: e.status() })?;
    let fs: &mut SimpleFileSystem = &mut *fs_scoped;

    // ── Ouverture du répertoire racine du volume ──────────────────────────────
    fs.open_volume()
        .map_err(|e| FileError::RootDirNotFound { status: e.status() })
}

/// Convertit une chaîne UTF-8 en UCS-2 null-terminé dans un buffer prédéfini.
/// Remplace `\` → `\` (les chemins UEFI utilisent `\` comme séparateur).
/// Retourne le nombre de caractères écrits (sans le null-terminator).
fn utf8_to_ucs2(src: &str, dst: &mut [u16]) -> Result<usize, ()> {
    let max_chars = dst.len().saturating_sub(1); // réserve pour \0
    let mut idx = 0usize;
    for ch in src.chars() {
        if idx >= max_chars { return Err(()); }
        // UCS-2 : seulement le BMP — les caractères hors BMP → '?'
        let c = if (ch as u32) < 0xD800 || ((ch as u32) > 0xDFFF && (ch as u32) < 0x10000) {
            ch as u16
        } else {
            b'?' as u16
        };
        // Normalisation du séparateur : '/' → '\' pour UEFI
        dst[idx] = if c == b'/' as u16 { b'\\' as u16 } else { c };
        idx += 1;
    }
    dst[idx] = 0; // null-terminator
    Ok(idx)
}

// ─── Buffer de fichier ────────────────────────────────────────────────────────

/// Buffer contenant les données d'un fichier chargé depuis l'ESP.
/// La mémoire est allouée dans le pool UEFI (LOADER_DATA).
///
/// ATTENTION : Ne pas utiliser après ExitBootServices sans avoir copié
/// les données dans une zone mémoire explicitement protégée (ex: pages LOADER_DATA).
pub struct FileBuffer {
    ptr:  *mut u8,
    size: usize,
}

impl FileBuffer {
    /// Accès en lecture aux données du fichier.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY : ptr et size sont cohérents et la mémoire est valide
        // tant que les Boot Services sont actifs.
        unsafe { core::slice::from_raw_parts(self.ptr, self.size) }
    }

    /// Adresse physique du buffer (identique au pointeur en UEFI flat memory model).
    #[inline]
    pub fn phys_addr(&self) -> u64 {
        self.ptr as u64
    }

    /// Taille du fichier en octets.
    #[inline]
    pub fn len(&self) -> usize { self.size }

    /// `true` si le fichier est vide (ne devrait pas arriver — FileError::Empty).
    #[inline]
    pub fn is_empty(&self) -> bool { self.size == 0 }
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum FileError<'p> {
    LoadedImageProtocolNotFound,
    SimpleFileSystemNotFound  { status: uefi::Status },
    DeviceHandleNull,
    RootDirNotFound           { status: uefi::Status },
    PathTooLong               { path: &'p str, max_chars: usize },
    NotFound                  { path: &'p str, status: uefi::Status },
    OpenFailed                { path: &'p str, status: uefi::Status },
    IsDirectory               { path: &'p str },
    InfoFailed                { path: &'p str, status: uefi::Status },
    Empty                     { path: &'p str },
    TooLarge                  { path: &'p str, size: u64, limit: u64 },
    AllocationFailed          { size: usize, status: uefi::Status },
    ReadFailed                { path: &'p str, status: uefi::Status },
    PartialRead               { path: &'p str, expected: usize, got: usize },
}

impl<'p> core::fmt::Display for FileError<'p> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFound { path, status } =>
                write!(f, "Fichier '{}' introuvable sur ESP : {:?}", path, status),
            Self::TooLarge { path, size, limit } =>
                write!(f, "Fichier '{}' trop grand : {} > {} bytes", path, size, limit),
            Self::PartialRead { path, expected, got } =>
                write!(f, "Lecture partielle '{}' : {} / {} bytes", path, got, expected),
            other =>
                write!(f, "FileError : {:?}", other),
        }
    }
}
