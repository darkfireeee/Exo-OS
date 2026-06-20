//! kernel-signer — génération de clé + signature de l'ELF kernel Exo-OS.
//!
//! Sous-commandes :
//!   keygen [--force] [--seed P] [--pubkey-rs P]
//!       Génère une paire Ed25519. Privée → `.secrets/kernel_signing.seed`
//!       (0600, gitignored). Publique → fichier Rust embarqué par le bootloader.
//!       Refuse d'écraser une graine existante sans `--force`.
//!   sign <kernel.elf> [--seed P]
//!       Signe l'ELF (footer EXOSIG01 = Ed25519 sur SHA-512 du corps). Idempotent
//!       (retire un footer existant avant de re-signer).
//!   verify <kernel.elf> [--seed P]
//!       Vérifie l'ELF avec la clé dérivée de la graine. Code retour ≠ 0 si pas
//!       `Verified` (utile en CI).
//!
//! Tout passe par la crate PARTAGÉE `exo-verity` → le signataire et le
//! vérificateur (bootloader) utilisent EXACTEMENT le même format et la même
//! logique : pas de divergence possible.

use std::fs;
use std::path::Path;
use std::process::exit;

use exo_verity::{
    key_is_usable, public_key_from_seed, sign_image, verify_image, KernelVerdict, SIG_FOOTER_SIZE,
    SIG_MARKER,
};

const DEFAULT_SEED_PATH: &str = ".secrets/kernel_signing.seed";
const DEFAULT_PUBKEY_RS: &str = "exo-boot/src/kernel_loader/signing_key.rs";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("");
    let rest = &args[args.len().min(2)..];
    let code = match cmd {
        "keygen" => cmd_keygen(rest),
        "sign" => cmd_sign(rest),
        "verify" => cmd_verify(rest),
        _ => {
            usage();
            2
        }
    };
    exit(code);
}

fn usage() {
    eprintln!(
        "kernel-signer — signature de l'ELF kernel Exo-OS (Ed25519, exo-verity)\n\
         \n\
         Usage:\n\
         \x20 kernel-signer keygen [--force] [--seed P] [--pubkey-rs P]\n\
         \x20 kernel-signer sign   <kernel.elf> [--seed P]\n\
         \x20 kernel-signer verify <kernel.elf> [--seed P]\n"
    );
}

/// Récupère la valeur d'une option `--name value` dans `args`.
fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Premier argument positionnel (qui ne commence pas par `--` et n'est pas une valeur d'option).
fn positional(args: &[String]) -> Option<&str> {
    let mut skip_next = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a.starts_with("--") {
            // les options de cet outil prennent toutes une valeur (sauf --force)
            if a != "--force" {
                skip_next = true;
            }
            continue;
        }
        return Some(a);
    }
    None
}

// ─── keygen ───────────────────────────────────────────────────────────────────

fn cmd_keygen(args: &[String]) -> i32 {
    let seed_path = opt(args, "--seed").unwrap_or(DEFAULT_SEED_PATH);
    let pubkey_rs = opt(args, "--pubkey-rs").unwrap_or(DEFAULT_PUBKEY_RS);
    let force = has_flag(args, "--force");

    if Path::new(seed_path).exists() && !force {
        eprintln!(
            "refus : la graine '{}' existe déjà (utilisez --force pour la remplacer — \
             ATTENTION : invalide toutes les images déjà signées).",
            seed_path
        );
        return 1;
    }

    // Génère une graine exploitable (boucle de sûreté — collision avec un vecteur
    // de test est astronomiquement improbable, mais on garantit l'invariant).
    let mut seed = [0u8; 32];
    loop {
        if let Err(e) = getrandom::getrandom(&mut seed) {
            eprintln!("échec getrandom : {e}");
            return 1;
        }
        if key_is_usable(&public_key_from_seed(&seed)) {
            break;
        }
    }
    let pubkey = public_key_from_seed(&seed);

    // Écrit la graine privée (création du dossier parent, perms 0600).
    if let Some(parent) = Path::new(seed_path).parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("création de '{}' impossible : {e}", parent.display());
            return 1;
        }
    }
    if let Err(e) = fs::write(seed_path, seed) {
        eprintln!("écriture de la graine '{}' impossible : {e}", seed_path);
        return 1;
    }
    restrict_permissions(seed_path);

    // Écrit le fichier Rust de clé publique embarqué par le bootloader.
    let rs = render_pubkey_rs(&pubkey);
    if let Err(e) = fs::write(pubkey_rs, rs) {
        eprintln!("écriture de la clé publique '{}' impossible : {e}", pubkey_rs);
        return 1;
    }

    println!("clé générée :");
    println!("  privée  : {seed_path} (0600, gitignored — NE JAMAIS committer)");
    println!("  publique: {pubkey_rs}");
    println!("  pubkey  : {}", hex(&pubkey));
    0
}

fn render_pubkey_rs(pubkey: &[u8; 32]) -> String {
    let mut body = String::new();
    for (i, b) in pubkey.iter().enumerate() {
        if i % 8 == 0 {
            body.push_str("\n    ");
        } else {
            body.push(' ');
        }
        body.push_str(&format!("0x{:02x},", b));
    }
    format!(
        "//! signing_key.rs — GÉNÉRÉ par `tools/kernel_signer` (keygen). NE PAS ÉDITER.\n\
         //!\n\
         //! Clé PUBLIQUE Ed25519 de vérification du kernel, embarquée dans le\n\
         //! bootloader et consommée par `exo-verity::verify_image`. La clé PRIVÉE\n\
         //! correspondante est dans `.secrets/kernel_signing.seed` (gitignored).\n\
         //! Régénérer : `cargo run -p exo-kernel-signer -- keygen --force`.\n\
         \n\
         /// Clé publique de signature kernel (32 octets, Ed25519).\n\
         pub const KERNEL_SIGNING_PUBLIC_KEY: [u8; 32] = [{body}\n];\n"
    )
}

// ─── sign ───────────────────────────────────────────────────────────────────

fn cmd_sign(args: &[String]) -> i32 {
    let Some(elf_path) = positional(args) else {
        eprintln!("sign : chemin de l'ELF kernel manquant");
        return 2;
    };
    let seed_path = opt(args, "--seed").unwrap_or(DEFAULT_SEED_PATH);

    let seed = match read_seed(seed_path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let elf = match fs::read(elf_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("lecture de '{}' impossible : {e}", elf_path);
            return 1;
        }
    };

    // Idempotence : retire un footer EXOSIG01 existant avant de re-signer.
    let body = strip_existing_footer(&elf);

    match sign_image(body, &seed) {
        Ok(signed) => {
            if let Err(e) = fs::write(elf_path, &signed) {
                eprintln!("écriture de '{}' impossible : {e}", elf_path);
                return 1;
            }
            println!(
                "signé : {} ({} octets corps + {} footer)",
                elf_path,
                body.len(),
                SIG_FOOTER_SIZE
            );
            0
        }
        Err(e) => {
            eprintln!("signature impossible : {e:?}");
            1
        }
    }
}

// ─── verify ─────────────────────────────────────────────────────────────────

fn cmd_verify(args: &[String]) -> i32 {
    let Some(elf_path) = positional(args) else {
        eprintln!("verify : chemin de l'ELF kernel manquant");
        return 2;
    };
    let seed_path = opt(args, "--seed").unwrap_or(DEFAULT_SEED_PATH);
    let seed = match read_seed(seed_path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let pubkey = public_key_from_seed(&seed);
    let elf = match fs::read(elf_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("lecture de '{}' impossible : {e}", elf_path);
            return 1;
        }
    };
    let verdict = verify_image(&elf, &pubkey);
    println!("{} : {}", elf_path, verdict.as_str());
    if verdict == KernelVerdict::Verified {
        0
    } else {
        1
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────────

fn read_seed(path: &str) -> Result<[u8; 32], i32> {
    let bytes = fs::read(path).map_err(|e| {
        eprintln!(
            "graine '{}' introuvable : {e}\n  → générez-la : cargo run -p exo-kernel-signer -- keygen",
            path
        );
        1
    })?;
    if bytes.len() != 32 {
        eprintln!("graine '{}' invalide : {} octets (32 attendus)", path, bytes.len());
        return Err(1);
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(seed)
}

fn strip_existing_footer(elf: &[u8]) -> &[u8] {
    if elf.len() >= SIG_FOOTER_SIZE {
        let start = elf.len() - SIG_FOOTER_SIZE;
        let mut marker = [0u8; 8];
        marker.copy_from_slice(&elf[start..start + 8]);
        if marker == SIG_MARKER {
            return &elf[..start];
        }
    }
    elf
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(unix)]
fn restrict_permissions(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &str) {
    // Hôte non-Unix : pas de chmod POSIX. La protection repose sur .gitignore.
}
