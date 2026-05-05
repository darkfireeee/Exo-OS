// kernel/build.rs — Script de build du noyau Exo-OS
//
// Responsabilités :
//  1. Passe le linker script `linker.ld` à rust-lld via cargo:rustc-link-arg.
//  2. Déclare les fichiers qui, en cas de modification, déclenchent un rebuild.
//  3. Produit le contrat ExoPhoenix Kernel A:
//     - image ELF propre embarquée dans OUT_DIR,
//     - hash BLAKE3 complet de l'image,
//     - racine d'intégrité BLAKE3(.text || .rodata).
//
// Pas besoin du crate `cc` : les fichiers .s sont inclus via `global_asm!(include_str!())`
// directement dans les fichiers Rust concernés (switch.rs pour switch_asm.s).

use std::path::{Path, PathBuf};

const ZERO_HASH: [u8; 32] = [0u8; 32];

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + (b - b'a')),
        b'A'..=b'F' => Some(10 + (b - b'A')),
        _ => None,
    }
}

fn parse_hash32_hex(var_name: &str, warn_missing: bool) -> Option<[u8; 32]> {
    println!("cargo:rerun-if-env-changed={var_name}");

    let Ok(raw) = std::env::var(var_name) else {
        if warn_missing {
            println!(
                "cargo:warning={var_name} non défini — fallback hash nul (mode dégradé ExoPhoenix)"
            );
        }
        return None;
    };

    let bytes = raw.as_bytes();
    if bytes.len() != 64 {
        println!(
            "cargo:warning={var_name} invalide (len={}) — attendu 64 hex chars, fallback hash nul",
            bytes.len()
        );
        return None;
    }

    let mut out = [0u8; 32];
    let mut i = 0usize;
    while i < 32 {
        let hi = hex_nibble(bytes[i * 2]);
        let lo = hex_nibble(bytes[i * 2 + 1]);
        let (Some(hi), Some(lo)) = (hi, lo) else {
            println!(
                "cargo:warning={var_name} contient des caractères non-hex — fallback hash nul"
            );
            return None;
        };
        out[i] = (hi << 4) | lo;
        i += 1;
    }

    Some(out)
}

fn read_u16_le(image: &[u8], off: usize) -> Result<u16, String> {
    let bytes = image
        .get(off..off + 2)
        .ok_or_else(|| format!("ELF tronqué: u16 @{off}"))?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u32_le(image: &[u8], off: usize) -> Result<u32, String> {
    let bytes = image
        .get(off..off + 4)
        .ok_or_else(|| format!("ELF tronqué: u32 @{off}"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u64_le(image: &[u8], off: usize) -> Result<u64, String> {
    let bytes = image
        .get(off..off + 8)
        .ok_or_else(|| format!("ELF tronqué: u64 @{off}"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn checked_usize(v: u64, field: &str) -> Result<usize, String> {
    usize::try_from(v).map_err(|_| format!("ELF {field} hors limites usize"))
}

fn section_header_off(
    image_len: usize,
    shoff: usize,
    shentsize: usize,
    index: usize,
) -> Result<usize, String> {
    let rel = index
        .checked_mul(shentsize)
        .ok_or_else(|| "ELF section header overflow".to_owned())?;
    let off = shoff
        .checked_add(rel)
        .ok_or_else(|| "ELF section header overflow".to_owned())?;
    let end = off
        .checked_add(64)
        .ok_or_else(|| "ELF section header overflow".to_owned())?;
    if end > image_len {
        return Err(format!("ELF section header {index} hors image"));
    }
    Ok(off)
}

fn section_body<'a>(
    image: &'a [u8],
    header_off: usize,
    section_name: &str,
) -> Result<&'a [u8], String> {
    let off = checked_usize(read_u64_le(image, header_off + 24)?, "sh_offset")?;
    let size = checked_usize(read_u64_le(image, header_off + 32)?, "sh_size")?;
    let end = off
        .checked_add(size)
        .ok_or_else(|| format!("ELF section {section_name} overflow"))?;
    image
        .get(off..end)
        .ok_or_else(|| format!("ELF section {section_name} hors image"))
}

fn section_name<'a>(shstr: &'a [u8], name_off: usize) -> Result<&'a str, String> {
    if name_off >= shstr.len() {
        return Err("ELF nom de section hors .shstrtab".to_owned());
    }
    let rest = &shstr[name_off..];
    let len = rest
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| "ELF nom de section non terminé".to_owned())?;
    std::str::from_utf8(&rest[..len]).map_err(|_| "ELF nom de section non UTF-8".to_owned())
}

fn compute_kernel_a_merkle_root(image: &[u8]) -> Result<[u8; 32], String> {
    if image.len() < 64 || image.get(0..4) != Some(b"\x7FELF") {
        return Err("Kernel A n'est pas un ELF valide".to_owned());
    }
    if image[4] != 2 || image[5] != 1 {
        return Err("Kernel A doit être ELF64 little-endian".to_owned());
    }

    let shoff = checked_usize(read_u64_le(image, 40)?, "e_shoff")?;
    let shentsize = usize::from(read_u16_le(image, 58)?);
    let shnum = usize::from(read_u16_le(image, 60)?);
    let shstrndx = usize::from(read_u16_le(image, 62)?);
    if shentsize < 64 || shnum == 0 || shstrndx >= shnum {
        return Err("table de sections ELF invalide".to_owned());
    }

    let shstr_hdr = section_header_off(image.len(), shoff, shentsize, shstrndx)?;
    let shstr = section_body(image, shstr_hdr, ".shstrtab")?;
    let mut text: Option<&[u8]> = None;
    let mut rodata: Option<&[u8]> = None;

    for index in 0..shnum {
        let hdr = section_header_off(image.len(), shoff, shentsize, index)?;
        let name_off = usize::try_from(read_u32_le(image, hdr)?)
            .map_err(|_| "ELF sh_name hors limites usize".to_owned())?;
        let name = section_name(shstr, name_off)?;
        match name {
            ".text" => text = Some(section_body(image, hdr, ".text")?),
            ".rodata" => rodata = Some(section_body(image, hdr, ".rodata")?),
            _ => {}
        }
    }

    let text = text.ok_or_else(|| "Kernel A ELF sans section .text".to_owned())?;
    let rodata = rodata.ok_or_else(|| "Kernel A ELF sans section .rodata".to_owned())?;
    if text.is_empty() || rodata.is_empty() {
        return Err("Kernel A ELF .text/.rodata vide".to_owned());
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(text);
    hasher.update(rodata);
    Ok(*hasher.finalize().as_bytes())
}

fn write_hash_blob(path: &Path, bytes: &[u8; 32]) {
    std::fs::write(path, bytes).expect("écriture hash ExoPhoenix OUT_DIR");
}

fn write_artifacts(out: &Path, image: &[u8], image_hash: &[u8; 32], merkle_root: &[u8; 32]) {
    write_hash_blob(&out.join("kernel_a_image_hash.bin"), image_hash);
    write_hash_blob(&out.join("kernel_a_merkle_root.bin"), merkle_root);
    std::fs::write(out.join("kernel_a_image.bin"), image)
        .expect("écriture image Kernel A ExoPhoenix OUT_DIR");
}

fn kernel_a_image_path_from_env() -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed=KERNEL_A_IMAGE_PATH");
    std::env::var_os("KERNEL_A_IMAGE_PATH").map(PathBuf::from)
}

fn main() {
    // Répertoire du crate (chemin absolu fourni par Cargo)
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = std::env::var("TARGET").unwrap_or_default();
    let linker_script = Path::new(&dir)
        .parent()
        .expect("workspace root manquant")
        .join("exo-boot")
        .join("linker")
        .join("linker.ld");
    let linker_script = linker_script
        .to_str()
        .expect("chemin linker non UTF-8")
        .to_owned();

    // Passer le linker script à rust-lld uniquement pour les cibles bare-metal.
    // Le chemin DOIT être absolu : le linker est invoqué depuis un répertoire temporaire.
    if target.contains("none") || target.contains("exo") {
        println!("cargo:rustc-link-arg=-T{linker_script}");
    }

    // Rebuild si le linker script change.
    println!("cargo:rerun-if-changed={linker_script}");

    // Rebuild si un fichier ASM change (les .s sont inclus via include_str!).
    println!("cargo:rerun-if-changed={dir}/src/scheduler/asm/switch_asm.s");
    println!("cargo:rerun-if-changed={dir}/src/scheduler/asm/fast_path.s");
    println!("cargo:rerun-if-changed={dir}/src/ipc/core/fastcall_asm.s");
    println!("cargo:rerun-if-env-changed=EXOPHOENIX_BUILD_ROLE");
    println!("cargo:rerun-if-env-changed=EXOPHOENIX_RESCUE_TEST");
    println!("cargo:rustc-check-cfg=cfg(exophoenix_resurrection_test)");

    if matches!(std::env::var("EXOPHOENIX_RESCUE_TEST").as_deref(), Ok("1")) {
        println!("cargo:rustc-cfg=exophoenix_resurrection_test");
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR manquant");
    let out = PathBuf::from(out_dir);
    let build_role = std::env::var("EXOPHOENIX_BUILD_ROLE").unwrap_or_default();
    let is_kernel_a_pass = build_role.eq_ignore_ascii_case("A");

    if let Some(image_path) = kernel_a_image_path_from_env() {
        println!("cargo:rerun-if-changed={}", image_path.display());
        let image = std::fs::read(&image_path)
            .unwrap_or_else(|err| panic!("lecture KERNEL_A_IMAGE_PATH impossible: {err}"));
        let image_hash = *blake3::hash(&image).as_bytes();
        let merkle_root = compute_kernel_a_merkle_root(&image)
            .unwrap_or_else(|err| panic!("contrat ExoPhoenix Kernel A invalide: {err}"));
        write_artifacts(&out, &image, &image_hash, &merkle_root);
        return;
    }

    if is_kernel_a_pass {
        // Première passe A: l'image propre est produite, puis la deuxième passe B
        // l'injecte avec ses hashes. Aucun warning dégradé ici: c'est volontaire.
        write_artifacts(&out, &[], &ZERO_HASH, &ZERO_HASH);
        return;
    }

    // Compatibilité ancienne pipeline: hashes fournis explicitement mais pas
    // d'image embarquée. Le forge refusera quand même la résurrection complète.
    let image_hash = parse_hash32_hex("KERNEL_A_IMAGE_HASH", true).unwrap_or(ZERO_HASH);
    let merkle_root = parse_hash32_hex("KERNEL_A_MERKLE_ROOT", true).unwrap_or(ZERO_HASH);
    write_artifacts(&out, &[], &image_hash, &merkle_root);
}
