//! tar_compat.rs — Émetteur de format POSIX tar pour compatibilité (no_std).
//!
//! Génère des headers POSIX ustar (512 bytes par entrée) permettant l'import
//! des blobs ExoFS dans des outils Unix standard.

use crate::fs::exofs::core::{BlobId, FsError};

const TAR_BLOCK_SIZE: usize = 512;

/// En-tête POSIX ustar (512 octets).
#[repr(C)]
pub struct TarHeader {
    pub name:     [u8; 100],
    pub mode:     [u8; 8],
    pub uid:      [u8; 8],
    pub gid:      [u8; 8],
    pub size:     [u8; 12],   // Taille en octal ASCII.
    pub mtime:    [u8; 12],   // mtime epoch en octal ASCII.
    pub checksum: [u8; 8],
    pub typeflag: u8,
    pub linkname: [u8; 100],
    pub magic:    [u8; 6],    // "ustar\0"
    pub version:  [u8; 2],   // "00"
    pub uname:    [u8; 32],
    pub gname:    [u8; 32],
    pub devmajor: [u8; 8],
    pub devminor: [u8; 8],
    pub prefix:   [u8; 155],
    pub _pad:     [u8; 12],
}

const _: () = assert!(core::mem::size_of::<TarHeader>() == TAR_BLOCK_SIZE);

pub trait TarSink {
    fn write_block(&mut self, block: &[u8; TAR_BLOCK_SIZE]) -> Result<(), FsError>;
}

pub struct TarEmitter;

impl TarEmitter {
    fn fill_octal(buf: &mut [u8], val: u64) {
        let s = buf.len();
        let mut v = val;
        for i in (0..s - 1).rev() {
            buf[i] = b'0' + (v & 7) as u8;
            v >>= 3;
        }
        buf[s - 1] = b' ';
    }

    fn blob_id_to_name(id: BlobId) -> [u8; 100] {
        // Nom = "exofs/<hex_blob_id>.blob"
        let mut name = [0u8; 100];
        let prefix = b"exofs/";
        name[..6].copy_from_slice(prefix);
        let bytes = id.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            let hi = b"0123456789abcdef"[(b >> 4) as usize];
            let lo = b"0123456789abcdef"[(b & 0xf) as usize];
            let off = 6 + i * 2;
            if off + 1 < 94 {
                name[off]     = hi;
                name[off + 1] = lo;
            }
        }
        let ext_off = 6 + 64;
        if ext_off + 5 < 100 {
            name[ext_off..ext_off + 5].copy_from_slice(b".blob");
        }
        name
    }

    fn compute_checksum(hdr: &TarHeader) -> u32 {
        // SAFETY: repr(C) 512B, lecture octet par octet.
        let bytes = unsafe {
            core::slice::from_raw_parts(hdr as *const _ as *const u8, TAR_BLOCK_SIZE)
        };
        bytes.iter().map(|&b| b as u32).sum()
    }

    /// Émet un header tar + les données d'un blob.
    /// Les données sont émises en blocs de 512 bytes (padding zéro).
    pub fn emit_blob(
        sink:   &mut dyn TarSink,
        id:     BlobId,
        data:   &[u8],
        mtime:  u64,
    ) -> Result<(), FsError> {
        let mut hdr = TarHeader {
            name:     Self::blob_id_to_name(id),
            mode:     *b"0000644\0",
            uid:      *b"0000000\0",
            gid:      *b"0000000\0",
            size:     [0u8; 12],
            mtime:    [0u8; 12],
            checksum: *b"        ",   // Rempli après calcul.
            typeflag: b'0',
            linkname: [0u8; 100],
            magic:    *b"ustar\0",
            version:  *b"00",
            uname:    [0u8; 32],
            gname:    [0u8; 32],
            devmajor: *b"0000000\0",
            devminor: *b"0000000\0",
            prefix:   [0u8; 155],
            _pad:     [0u8; 12],
        };
        Self::fill_octal(&mut hdr.size, data.len() as u64);
        Self::fill_octal(&mut hdr.mtime, mtime);

        // Calcul checksum avec checksum = " " × 8.
        let ck = Self::compute_checksum(&hdr);
        let mut ckbuf = [0u8; 8];
        Self::fill_octal(&mut ckbuf[..7], ck as u64);
        ckbuf[7] = 0;
        hdr.checksum.copy_from_slice(&ckbuf);

        // Émission header.
        // SAFETY: repr(C) 512B.
        let hdr_block: &[u8; TAR_BLOCK_SIZE] = unsafe {
            &*((&hdr as *const TarHeader) as *const [u8; TAR_BLOCK_SIZE])
        };
        sink.write_block(hdr_block)?;

        // Émission données par blocs.
        let mut off = 0usize;
        while off < data.len() {
            let mut block = [0u8; TAR_BLOCK_SIZE];
            let end = (off + TAR_BLOCK_SIZE).min(data.len());
            block[..end - off].copy_from_slice(&data[off..end]);
            sink.write_block(&block)?;
            off += TAR_BLOCK_SIZE;
        }
        Ok(())
    }

    /// Émet les deux blocs de fin d'archive tar (zéros).
    pub fn emit_eof(sink: &mut dyn TarSink) -> Result<(), FsError> {
        let block = [0u8; TAR_BLOCK_SIZE];
        sink.write_block(&block)?;
        sink.write_block(&block)
    }
}
