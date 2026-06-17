use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEVICE_BLOCK_SIZE: u64 = 4096;
const EXOFS_BLOCK_SIZE: u64 = DEVICE_BLOCK_SIZE;
const MIN_DISK_SIZE: u64 = 16 * 1024 * 1024;
const HEAP_START_OFFSET: u64 = 1024 * 1024;
const DATA_LBA_START: u64 = 2048;
const RESERVED_LBA_END: u64 = 0x0301;
const OBJECT_INDEX_LBA: u64 = 64;
const OBJECT_INDEX_BLOCKS: u64 = 128;
const OBJECT_INDEX_MAGIC: u32 = 0x4558_4F49;
const OBJECT_INDEX_VERSION: u16 = 1;
const OBJECT_INDEX_HEADER_SIZE: usize = 32;
const OBJECT_INDEX_ENTRY_SIZE: usize = 64;
const EXOFS_MAGIC: u32 = 0x4558_4F46;
const PATH_INDEX_MAGIC: u32 = 0x5049_4458;
const PATH_INDEX_VERSION: u16 = 1;
const PATH_INDEX_HEADER_SIZE: usize = 148;
const PATH_INDEX_ENTRY_SIZE: usize = 44;
const PATH_INDEX_SPLIT_THRESHOLD: u32 = 192;
const PATH_INDEX_KIND_DIR: u8 = 0;
const PATH_INDEX_KIND_FILE: u8 = 1;
const INCOMPAT_REQUIRED: u64 = (1 << 3) | (1 << 4) | (1 << 5);
const SUPERBLOCK_SIZE: usize = 512;
const DEFAULT_VOLUME_NAME: &[u8] = b"ExoOS-root";

#[derive(Clone)]
struct InputFile {
    image_path: String,
    host_path: PathBuf,
}

struct Mapping {
    blob_id: [u8; 32],
    base_lba: u64,
    allocated_blocks: u64,
    size_bytes: u64,
}

struct PayloadBlob {
    image_path: String,
    data: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Directory,
    File,
}

impl NodeKind {
    fn path_index_kind(self) -> u8 {
        match self {
            Self::Directory => PATH_INDEX_KIND_DIR,
            Self::File => PATH_INDEX_KIND_FILE,
        }
    }
}

struct Args {
    image: PathBuf,
    root: PathBuf,
    size: u64,
    /// FIX-F1 : crée un volume CHIFFRÉ (blobs chiffrés + clé de volume wrappée
    /// dans le superblock). Requiert `--passphrase`.
    encrypt: bool,
    passphrase: Option<String>,
}

/// Flag incompat ENCRYPTION (doit matcher `incompat_flags::ENCRYPTION` du kernel).
const INCOMPAT_ENCRYPTION: u64 = 1 << 2;

fn main() {
    if let Err(err) = run() {
        eprintln!("exofs-mkroot: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    if args.size < MIN_DISK_SIZE {
        return Err(format!("image size must be at least {MIN_DISK_SIZE} bytes").into());
    }
    if args.size % DEVICE_BLOCK_SIZE != 0 {
        return Err("image size must be aligned to the ExoFS logical block size".into());
    }

    let files = collect_files(&args.root)?;
    if files.is_empty() {
        return Err(format!("no regular files found under {}", args.root.display()).into());
    }
    let payloads = build_payload_blobs(&files)?;

    if let Some(parent) = args.image.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut image = File::create(&args.image)?;
    image.set_len(args.size)?;

    // FIX-F1 : volume chiffré → générer la clé de volume (VK) et la wrapper
    // (Argon2id + AEAD via exo-fscrypt) pour stockage dans le superblock.
    let mut wrapped_vk: Option<[u8; exo_fscrypt::WRAPPED_VK_LEN]> = None;
    let mut volume_key: Option<[u8; 32]> = None;
    if args.encrypt {
        let pw = args.passphrase.as_deref().unwrap_or("");
        let mut vk = [0u8; 32];
        vk.copy_from_slice(&random_bytes(32)?);
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&random_bytes(32)?);
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&random_bytes(24)?);
        let w = exo_fscrypt::wrap_volume_key(&vk, pw.as_bytes(), &salt, &nonce)
            .map_err(|e| format!("volume-key wrap failed: {e:?}"))?;
        wrapped_vk = Some(w);
        volume_key = Some(vk);
    }

    let mappings = write_payload_blobs(&mut image, args.size, &payloads, volume_key.as_ref())?;
    write_object_catalog(&mut image, &mappings, args.size)?;
    write_superblocks(&mut image, args.size, mappings.len() as u64, wrapped_vk.as_ref())?;
    image.sync_all()?;

    println!(
        "ExoFS root image {}: {} blobs, {} bytes{}",
        args.image.display(),
        payloads.len(),
        payloads
            .iter()
            .map(|payload| payload.data.len() as u64)
            .sum::<u64>(),
        if args.encrypt { " [CHIFFRÉ]" } else { "" }
    );
    Ok(())
}

/// Lit `n` octets aléatoires depuis /dev/urandom (environnement de build Linux/WSL).
fn random_bytes(n: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    use std::io::Read;
    let mut f = File::open("/dev/urandom")?;
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = pico_args::Arguments::from_env();
    if args.contains(["-h", "--help"]) {
        println!(
            "usage: exofs-mkroot --image PATH --size 512M --root DIR \
             [--encrypt --passphrase PW]"
        );
        std::process::exit(0);
    }

    let encrypt = args.contains("--encrypt");
    let passphrase = args.opt_value_from_str::<_, String>("--passphrase")?;

    let image = args
        .opt_value_from_os_str("--image", |value| {
            Ok::<_, &'static str>(PathBuf::from(value))
        })?
        .ok_or("--image is required")?;
    let root = args
        .opt_value_from_os_str("--root", |value| {
            Ok::<_, &'static str>(PathBuf::from(value))
        })?
        .ok_or("--root is required")?;
    let raw_size = args
        .opt_value_from_str::<_, String>("--size")?
        .ok_or("--size is required")?;

    let remaining = args.finish();
    if !remaining.is_empty() {
        return Err(format!("unknown argument: {}", remaining[0].to_string_lossy()).into());
    }

    if encrypt && passphrase.is_none() {
        return Err("--encrypt requires --passphrase".into());
    }

    Ok(Args {
        image,
        root,
        size: parse_size(&raw_size)?,
        encrypt,
        passphrase,
    })
}

fn parse_size(raw: &str) -> Result<u64, Box<dyn Error>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty size".into());
    }
    let (digits, multiplier) = match trimmed.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&trimmed[..trimmed.len() - 1], 1024u64),
        Some(b'm' | b'M') => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024),
        Some(b'g' | b'G') => (&trimmed[..trimmed.len() - 1], 1024u64 * 1024 * 1024),
        _ => (trimmed, 1u64),
    };
    let base: u64 = digits.parse()?;
    base.checked_mul(multiplier)
        .ok_or_else(|| "size overflow".into())
}

fn collect_files(root: &Path) -> Result<Vec<InputFile>, Box<dyn Error>> {
    let mut out = Vec::new();
    collect_files_inner(root, root, &mut out)?;
    out.sort_by(|a, b| a.image_path.cmp(&b.image_path));
    Ok(out)
}

fn collect_files_inner(
    root: &Path,
    current: &Path,
    out: &mut Vec<InputFile>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_files_inner(root, &path, out)?;
        } else if ty.is_file() {
            let rel = path.strip_prefix(root)?;
            let image_path = image_path_for(rel)?;
            out.push(InputFile {
                image_path,
                host_path: path,
            });
        }
    }
    Ok(())
}

fn image_path_for(rel: &Path) -> Result<String, Box<dyn Error>> {
    let mut out = String::from("/");
    let mut first = true;
    for component in rel.components() {
        let text = component
            .as_os_str()
            .to_str()
            .ok_or("rootfs path must be valid UTF-8")?;
        if text.is_empty()
            || text == "."
            || text == ".."
            || text.contains('/')
            || text.contains('\\')
        {
            return Err(format!("invalid rootfs component: {text:?}").into());
        }
        if !first {
            out.push('/');
        }
        out.push_str(text);
        first = false;
    }
    if out == "/" {
        return Err("rootfs file path resolved to /".into());
    }
    Ok(out)
}

fn build_payload_blobs(files: &[InputFile]) -> Result<Vec<PayloadBlob>, Box<dyn Error>> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    let mut entries: BTreeMap<String, BTreeMap<String, NodeKind>> = BTreeMap::new();
    dirs.insert("/".to_string());
    entries.entry("/".to_string()).or_default();

    for file in files {
        let components = path_components(&file.image_path)?;
        if components.is_empty() {
            return Err("rootfs file path resolved to /".into());
        }

        let mut parent = "/".to_string();
        for (idx, component) in components.iter().enumerate() {
            let is_last = idx + 1 == components.len();
            let kind = if is_last {
                NodeKind::File
            } else {
                NodeKind::Directory
            };
            insert_dir_entry(&mut entries, &parent, component, kind)?;
            if !is_last {
                let child_dir = child_path(&parent, component);
                dirs.insert(child_dir.clone());
                entries.entry(child_dir.clone()).or_default();
                parent = child_dir;
            }
        }
    }

    for dir in &dirs {
        entries.entry(dir.clone()).or_default();
    }

    let mut payloads = Vec::new();
    payloads.try_reserve(entries.len() + files.len())?;
    for (dir, children) in &entries {
        payloads.push(PayloadBlob {
            image_path: dir.clone(),
            data: serialize_path_index(dir, children)?,
        });
    }
    for file in files {
        payloads.push(PayloadBlob {
            image_path: file.image_path.clone(),
            data: fs::read(&file.host_path)?,
        });
    }
    payloads.sort_by(|a, b| a.image_path.cmp(&b.image_path));
    Ok(payloads)
}

fn path_components(path: &str) -> Result<Vec<String>, Box<dyn Error>> {
    if !path.starts_with('/') {
        return Err(format!("rootfs path must be absolute: {path}").into());
    }
    let mut out = Vec::new();
    for raw in path.split('/').filter(|part| !part.is_empty()) {
        validate_component(raw.as_bytes())?;
        out.push(raw.to_string());
    }
    Ok(out)
}

fn child_path(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{parent}/{child}")
    }
}

fn insert_dir_entry(
    entries: &mut BTreeMap<String, BTreeMap<String, NodeKind>>,
    parent: &str,
    name: &str,
    kind: NodeKind,
) -> Result<(), Box<dyn Error>> {
    validate_component(name.as_bytes())?;
    let siblings = entries.entry(parent.to_string()).or_default();
    match siblings.get(name).copied() {
        Some(existing) if existing != kind => {
            Err(format!("rootfs path kind conflict at {parent}/{name}").into())
        }
        Some(_) => Ok(()),
        None => {
            siblings.insert(name.to_string(), kind);
            Ok(())
        }
    }
}

fn serialize_path_index(
    dir_path: &str,
    entries: &BTreeMap<String, NodeKind>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::new();
    let names_len = entries.keys().map(|name| name.len()).sum::<usize>();
    let total = PATH_INDEX_HEADER_SIZE
        .checked_add(
            entries
                .len()
                .checked_mul(PATH_INDEX_ENTRY_SIZE)
                .ok_or("path index size overflow")?,
        )
        .and_then(|value| value.checked_add(names_len))
        .ok_or("path index size overflow")?;
    out.try_reserve(total)?;

    push_u32(&mut out, PATH_INDEX_MAGIC);
    push_u16(&mut out, PATH_INDEX_VERSION);
    push_u16(&mut out, 0);
    out.extend_from_slice(&parent_oid_for_dir(dir_path));
    push_u32(&mut out, entries.len() as u32);
    out.extend_from_slice(&[0u8; 32]);
    out.extend_from_slice(&[0u8; 32]);
    push_u32(&mut out, PATH_INDEX_SPLIT_THRESHOLD);
    push_u32(&mut out, 0);
    out.extend_from_slice(&[0u8; 32]);

    for (name, kind) in entries {
        validate_component(name.as_bytes())?;
        push_u64(&mut out, fnv1a_hash(name.as_bytes()));
        out.extend_from_slice(&blake3_hash(child_path(dir_path, name).as_bytes()));
        push_u16(&mut out, name.len() as u16);
        out.push(kind.path_index_kind());
        out.push(0);
        out.extend_from_slice(name.as_bytes());
    }

    debug_assert_eq!(out.len(), total);
    Ok(out)
}

fn parent_oid_for_dir(dir_path: &str) -> [u8; 32] {
    if dir_path == "/" {
        [0u8; 32]
    } else {
        blake3_hash(parent_path(dir_path).as_bytes())
    }
}

fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(idx) => trimmed[..idx].to_string(),
    }
}

fn validate_component(bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    if bytes.is_empty() || bytes.len() > 255 {
        return Err("invalid rootfs path component length".into());
    }
    if bytes.iter().any(|&byte| byte == b'/' || byte == 0) {
        return Err("invalid rootfs path component byte".into());
    }
    std::str::from_utf8(bytes)?;
    Ok(())
}

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn write_payload_blobs(
    image: &mut File,
    image_size: u64,
    payloads: &[PayloadBlob],
    enc_vk: Option<&[u8; 32]>,
) -> Result<Vec<Mapping>, Box<dyn Error>> {
    let total_blocks = image_size / DEVICE_BLOCK_SIZE;
    let mut next_lba = data_lba_start(total_blocks);
    let mut mappings = Vec::with_capacity(payloads.len());

    for payload in payloads {
        let blocks = blocks_for(payload.data.len() as u64);
        let end_lba = next_lba
            .checked_add(blocks)
            .ok_or("LBA overflow while writing rootfs")?;
        if end_lba > total_blocks {
            return Err(format!("rootfs image is too small for {}", payload.image_path).into());
        }

        let offset = next_lba
            .checked_mul(DEVICE_BLOCK_SIZE)
            .ok_or("byte offset overflow while writing rootfs")?;
        // BlobId = blake3(chemin) — identique à la convention kernel.
        let blob_id = blake3_hash(payload.image_path.as_bytes());

        if let Some(vk) = enc_vk {
            // FIX-F1 : chiffrer chaque bloc de 4096 à son offset logique, EXACTEMENT
            // comme le chemin de lecture kernel (exo-fscrypt, cohérence garantie).
            let key = exo_fscrypt::blob_key(vk, &blob_id);
            let padded = (blocks * DEVICE_BLOCK_SIZE) as usize;
            let mut buf = vec![0u8; padded];
            buf[..payload.data.len()].copy_from_slice(&payload.data);
            let mut pos = 0usize;
            while pos < padded {
                let end = (pos + DEVICE_BLOCK_SIZE as usize).min(padded);
                exo_fscrypt::xor_block(&key, &blob_id, pos as u64, &mut buf[pos..end]);
                pos = end;
            }
            image.seek(SeekFrom::Start(offset))?;
            image.write_all(&buf)?;
        } else {
            write_padded(image, offset, &payload.data, blocks * DEVICE_BLOCK_SIZE)?;
        }

        mappings.push(Mapping {
            blob_id,
            base_lba: next_lba,
            allocated_blocks: blocks,
            size_bytes: payload.data.len() as u64,
        });
        next_lba = end_lba;
    }

    Ok(mappings)
}

fn data_lba_start(total_blocks: u64) -> u64 {
    if total_blocks > DATA_LBA_START {
        DATA_LBA_START
    } else if total_blocks > RESERVED_LBA_END {
        RESERVED_LBA_END
    } else if total_blocks > OBJECT_INDEX_LBA + OBJECT_INDEX_BLOCKS {
        OBJECT_INDEX_LBA + OBJECT_INDEX_BLOCKS
    } else {
        1
    }
}

fn blocks_for(size: u64) -> u64 {
    if size == 0 {
        0
    } else {
        (size + DEVICE_BLOCK_SIZE - 1) / DEVICE_BLOCK_SIZE
    }
}

fn write_padded(
    image: &mut File,
    offset: u64,
    data: &[u8],
    padded_len: u64,
) -> Result<(), Box<dyn Error>> {
    image.seek(SeekFrom::Start(offset))?;
    image.write_all(data)?;
    let padding = padded_len
        .checked_sub(data.len() as u64)
        .ok_or("padding underflow")?;
    write_zeroes(image, padding)?;
    Ok(())
}

fn write_zeroes(image: &mut File, mut len: u64) -> Result<(), Box<dyn Error>> {
    const ZEROES: [u8; 4096] = [0u8; 4096];
    while len > 0 {
        let chunk = len.min(ZEROES.len() as u64) as usize;
        image.write_all(&ZEROES[..chunk])?;
        len -= chunk as u64;
    }
    Ok(())
}

fn write_object_catalog(
    image: &mut File,
    mappings: &[Mapping],
    image_size: u64,
) -> Result<(), Box<dyn Error>> {
    let total = (OBJECT_INDEX_BLOCKS * DEVICE_BLOCK_SIZE) as usize;
    let used = OBJECT_INDEX_HEADER_SIZE + mappings.len() * OBJECT_INDEX_ENTRY_SIZE;
    if used > total {
        return Err("object catalog does not fit in reserved index area".into());
    }

    let mut buf = Vec::with_capacity(total);
    let next_lba = mappings
        .iter()
        .map(|mapping| mapping.base_lba + mapping.allocated_blocks)
        .max()
        .unwrap_or_else(|| data_lba_start(image_size / DEVICE_BLOCK_SIZE));

    push_u32(&mut buf, OBJECT_INDEX_MAGIC);
    push_u16(&mut buf, OBJECT_INDEX_VERSION);
    push_u16(&mut buf, 0);
    push_u64(&mut buf, next_lba);
    push_u32(&mut buf, mappings.len() as u32);
    push_u32(&mut buf, 0);
    push_u64(&mut buf, 0);

    for mapping in mappings {
        buf.extend_from_slice(&mapping.blob_id);
        push_u64(&mut buf, mapping.base_lba);
        push_u64(&mut buf, mapping.allocated_blocks);
        push_u64(&mut buf, mapping.size_bytes);
        push_u32(&mut buf, DEVICE_BLOCK_SIZE as u32);
        push_u32(&mut buf, 0);
    }
    buf.resize(total, 0);

    image.seek(SeekFrom::Start(OBJECT_INDEX_LBA * DEVICE_BLOCK_SIZE))?;
    image.write_all(&buf)?;
    Ok(())
}

fn write_superblocks(
    image: &mut File,
    image_size: u64,
    object_count: u64,
    wrapped_vk: Option<&[u8; exo_fscrypt::WRAPPED_VK_LEN]>,
) -> Result<(), Box<dyn Error>> {
    let sb = build_superblock(image_size, object_count, wrapped_vk)?;
    for offset in [0, 3 * EXOFS_BLOCK_SIZE, image_size - EXOFS_BLOCK_SIZE] {
        image.seek(SeekFrom::Start(offset))?;
        image.write_all(&sb)?;
    }
    Ok(())
}

fn build_superblock(
    image_size: u64,
    object_count: u64,
    wrapped_vk: Option<&[u8; exo_fscrypt::WRAPPED_VK_LEN]>,
) -> Result<[u8; SUPERBLOCK_SIZE], Box<dyn Error>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let heap_end = image_size - 2 * EXOFS_BLOCK_SIZE;
    let uuid_hash = blake3::hash(
        &[
            DEFAULT_VOLUME_NAME,
            &image_size.to_le_bytes(),
            &object_count.to_le_bytes(),
        ]
        .concat(),
    );
    let mut name = [0u8; 64];
    name[..DEFAULT_VOLUME_NAME.len()].copy_from_slice(DEFAULT_VOLUME_NAME);

    let mut out = [0u8; SUPERBLOCK_SIZE];
    let mut off = 0usize;
    let incompat = if wrapped_vk.is_some() {
        INCOMPAT_REQUIRED | INCOMPAT_ENCRYPTION
    } else {
        INCOMPAT_REQUIRED
    };
    put_u32(&mut out, &mut off, EXOFS_MAGIC);
    put_u16(&mut out, &mut off, 1);
    put_u16(&mut out, &mut off, 0);
    put_u64(&mut out, &mut off, incompat);
    put_u64(&mut out, &mut off, 0);
    put_u64(&mut out, &mut off, image_size);
    put_u64(&mut out, &mut off, HEAP_START_OFFSET);
    put_u64(&mut out, &mut off, heap_end);
    put_u64(&mut out, &mut off, 3 * EXOFS_BLOCK_SIZE);
    put_u64(&mut out, &mut off, image_size - EXOFS_BLOCK_SIZE);
    put_u64(&mut out, &mut off, now);
    put_bytes(&mut out, &mut off, &uuid_hash.as_bytes()[..16]);
    put_bytes(&mut out, &mut off, &name);
    put_u32(&mut out, &mut off, EXOFS_BLOCK_SIZE as u32);
    put_u8(&mut out, &mut off, 0);
    put_bytes(&mut out, &mut off, &[0; 3]);
    put_u64(&mut out, &mut off, object_count);
    put_u64(&mut out, &mut off, object_count);
    put_u64(
        &mut out,
        &mut off,
        heap_end.saturating_sub(HEAP_START_OFFSET),
    );
    put_u64(&mut out, &mut off, 1);
    put_u64(&mut out, &mut off, 0);
    put_u64(&mut out, &mut off, now);
    // FIX-F1 : _pad1[272] — les 110 premiers octets contiennent la clé de volume
    // wrappée (format exo-fscrypt) si le volume est chiffré ; le reste est réservé.
    let mut pad1 = [0u8; 272];
    if let Some(w) = wrapped_vk {
        pad1[..w.len()].copy_from_slice(w);
    }
    put_bytes(&mut out, &mut off, &pad1);
    debug_assert_eq!(off, SUPERBLOCK_SIZE - 32);
    let checksum = blake3_hash(&out[..SUPERBLOCK_SIZE - 32]);
    out[SUPERBLOCK_SIZE - 32..].copy_from_slice(&checksum);
    Ok(out)
}

fn blake3_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn put_u8(buf: &mut [u8], off: &mut usize, value: u8) {
    buf[*off] = value;
    *off += 1;
}

fn put_u16(buf: &mut [u8], off: &mut usize, value: u16) {
    put_bytes(buf, off, &value.to_le_bytes());
}

fn put_u32(buf: &mut [u8], off: &mut usize, value: u32) {
    put_bytes(buf, off, &value.to_le_bytes());
}

fn put_u64(buf: &mut [u8], off: &mut usize, value: u64) {
    put_bytes(buf, off, &value.to_le_bytes());
}

fn put_bytes(buf: &mut [u8], off: &mut usize, bytes: &[u8]) {
    let end = *off + bytes.len();
    buf[*off..end].copy_from_slice(bytes);
    *off = end;
}
