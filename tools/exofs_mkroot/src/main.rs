use std::env;
use std::error::Error;
use std::ffi::OsString;
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
const INCOMPAT_REQUIRED: u64 = (1 << 3) | (1 << 4) | (1 << 5);
const SUPERBLOCK_SIZE: usize = 512;
const DEFAULT_VOLUME_NAME: &[u8] = b"ExoOS-root";

#[derive(Clone)]
struct InputFile {
    image_path: String,
    host_path: PathBuf,
    size: u64,
}

struct Mapping {
    blob_id: [u8; 32],
    base_lba: u64,
    allocated_blocks: u64,
    size_bytes: u64,
}

struct Args {
    image: PathBuf,
    root: PathBuf,
    size: u64,
}

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

    if let Some(parent) = args.image.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut image = File::create(&args.image)?;
    image.set_len(args.size)?;

    let mappings = write_payload_files(&mut image, args.size, &files)?;
    write_object_catalog(&mut image, &mappings, args.size)?;
    write_superblocks(&mut image, args.size, mappings.len() as u64)?;
    image.sync_all()?;

    println!(
        "ExoFS root image {}: {} files, {} bytes",
        args.image.display(),
        files.len(),
        files.iter().map(|file| file.size).sum::<u64>()
    );
    Ok(())
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut image = None;
    let mut root = None;
    let mut size = None;
    let mut it = env::args_os().skip(1);

    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--image") => image = Some(PathBuf::from(next_arg_value(&mut it, "--image")?)),
            Some("--root") => root = Some(PathBuf::from(next_arg_value(&mut it, "--root")?)),
            Some("--size") => {
                let raw = next_arg_value(&mut it, "--size")?;
                let raw = raw.to_str().ok_or("--size must be valid UTF-8")?;
                size = Some(parse_size(raw)?);
            }
            Some("--help" | "-h") => {
                println!("usage: exofs-mkroot --image PATH --size 512M --root DIR");
                std::process::exit(0);
            }
            Some(other) => return Err(format!("unknown argument: {other}").into()),
            None => {
                return Err(format!(
                    "argument name must be valid UTF-8: {}",
                    arg.to_string_lossy()
                )
                .into());
            }
        }
    }

    Ok(Args {
        image: image.ok_or("--image is required")?,
        root: root.ok_or("--root is required")?,
        size: size.ok_or("--size is required")?,
    })
}

fn next_arg_value(
    it: &mut impl Iterator<Item = OsString>,
    name: &'static str,
) -> Result<OsString, Box<dyn Error>> {
    it.next()
        .ok_or_else(|| format!("{name} requires a value").into())
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
            let size = entry.metadata()?.len();
            out.push(InputFile {
                image_path,
                host_path: path,
                size,
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

fn write_payload_files(
    image: &mut File,
    image_size: u64,
    files: &[InputFile],
) -> Result<Vec<Mapping>, Box<dyn Error>> {
    let total_blocks = image_size / DEVICE_BLOCK_SIZE;
    let mut next_lba = data_lba_start(total_blocks);
    let mut mappings = Vec::with_capacity(files.len());

    for file in files {
        let data = fs::read(&file.host_path)?;
        let blocks = blocks_for(data.len() as u64);
        let end_lba = next_lba
            .checked_add(blocks)
            .ok_or("LBA overflow while writing rootfs")?;
        if end_lba > total_blocks {
            return Err(format!("rootfs image is too small for {}", file.image_path).into());
        }

        let offset = next_lba
            .checked_mul(DEVICE_BLOCK_SIZE)
            .ok_or("byte offset overflow while writing rootfs")?;
        write_padded(image, offset, &data, blocks * DEVICE_BLOCK_SIZE)?;

        mappings.push(Mapping {
            blob_id: blake3_hash(file.image_path.as_bytes()),
            base_lba: next_lba,
            allocated_blocks: blocks,
            size_bytes: data.len() as u64,
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
) -> Result<(), Box<dyn Error>> {
    let sb = build_superblock(image_size, object_count)?;
    for offset in [0, 3 * EXOFS_BLOCK_SIZE, image_size - EXOFS_BLOCK_SIZE] {
        image.seek(SeekFrom::Start(offset))?;
        image.write_all(&sb)?;
    }
    Ok(())
}

fn build_superblock(
    image_size: u64,
    object_count: u64,
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
    put_u32(&mut out, &mut off, EXOFS_MAGIC);
    put_u16(&mut out, &mut off, 1);
    put_u16(&mut out, &mut off, 0);
    put_u64(&mut out, &mut off, INCOMPAT_REQUIRED);
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
    put_bytes(&mut out, &mut off, &[0; 272]);
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
