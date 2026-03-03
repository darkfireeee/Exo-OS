EXO-OS · ExoFS v3 + security/
Implémentation de la Sécurité
Capabilities · Zero Trust · Crypto · Audit · ObjectKind · Lacunes
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Analyse croisée ExoFS v3 × DOC1-10 × Architecture v6 × Audits Gemini/Z-AI

1 — Architecture de la Sécurité dans ExoOS
La sécurité d'ExoOS repose sur deux piliers qui doivent fonctionner ensemble sans se dupliquer.

1.1 — Frontière des responsabilités
┌─────────────────────────────────────────────────────────┐
│  security/ (Ring 0 — couche TCB)                        │
│  Responsabilité : VÉRIFICATION des droits               │
│  ├── capability/verify.rs   ← point d'entrée UNIQUE     │
│  ├── capability/table.rs    ← CapTable par processus    │
│  ├── capability/rights.rs   ← bitflags Rights           │
│  ├── access_control/        ← check_access() (v6)       │
│  └── crypto/                ← primitives no_std Ring0   │
└─────────────────────────────────────────────────────────┘
              ↑ verify_cap() appelé par ↓
┌─────────────────────────────────────────────────────────┐
│  fs/exofs/ (Ring 0 — couche 3)                          │
│  Responsabilité : UTILISATION des droits                │
│  ├── crypto/xchacha20.rs    ← chiffrement L-Obj Secret  │
│  ├── crypto/blake3.rs       ← BlobId + checksums        │
│  ├── crypto/key_derivation  ← HKDF volume/object keys   │
│  ├── audit/                 ← ring buffer audit         │
│  └── quota/                 ← limites par capability    │
└─────────────────────────────────────────────────────────┘
              ↑ utilisé via IPC par ↓
┌─────────────────────────────────────────────────────────┐
│  servers/posix_server/ (Ring 1)                         │
│  Jamais : modifier CapTokens kernel                     │
│  Jamais : accès direct aux structures Ring0             │
└─────────────────────────────────────────────────────────┘

1.2 — Règle d'or : Zero Trust inviolable
🔴 SEC-ZERO — TOUT accès à un objet ExoFS passe par verify_cap(cap, object_id, rights). Sans exception. Aucun bypass possible, même pour le GC, le writeback ou le recovery.
🔴 SEC-UNIQUE — security/capability/verify() = point d'entrée UNIQUE. INTERDIT de réimplémenter hors de security/capability/. CI grep automatique vérifie que personne ne bypasse.
🔴 SEC-V6 — ipc/ et fs/exofs/ appellent security::access_control::check_access() DIRECTEMENT (v6). ipc/capability_bridge/ est supprimé.

2 — Système de Capabilities
2.1 — Principes
✅ CAP-01 — Vérification O(1) : token.generation == table[object_id].generation. Pas de parcours, pas de liste.
✅ CAP-02 — Révocation = increment atomique de la generation du slot. Tous les tokens existants deviennent invalides instantanément.
🔴 CAP-03 — Délégation : droits_délégués ⊆ droits_délégateur. Impossible d'obtenir plus de droits que ce qu'on possède.
🔴 CAP-04 — ObjectId Class2 = compteur global monotone, jamais réinitialisé. Empêche la réutilisation d'identifiants après suppression.
🔴 CAP-05 — verify() doit être constant-time — pas d'early return qui révèle si l'objet existe ou est révoqué (timing side-channel).
🔴 CAP-06 — do_exit() : cap_table.revoke_all() — TOUTES les caps révoquées, sans exception.
✅ CAP-07 — do_exec() : FD_CLOEXEC révoqué auto. ExecCapPolicy appliquée (Inherit/Revoke/Ambient).
✅ CAP-08 — fork() : cap table dupliquée (ref_count caps partagées). Caps FD héritées, mémoire CoW, IPC clonées.

2.2 — verify() correct (constant-time)
// ❌ FAUTIF — early return révèle l'existence de l'objet (timing side-channel)
pub fn verify(table: &CapTable, token: CapToken,
              rights: Rights) -> Result<(), CapError> {
    let entry = table.get(token.object_id)
        .ok_or(CapError::ObjectNotFound)?;  // ← retour rapide si absent
    if entry.generation != token.generation {
        return Err(CapError::Revoked);      // ← timing différent de NotFound
    }
    if !entry.rights.contains(rights) {
        return Err(CapError::Denied);
    }
    Ok(())
}
 
// ✅ CORRECT — parcours complet même si objet absent
pub fn verify(table: &CapTable, token: CapToken,
              rights: Rights) -> Result<(), CapError> {
    let entry_opt = table.get(token.object_id);
    // Comparaison constante — même si entry_opt = None
    let stored_gen  = entry_opt.map(|e| e.generation).unwrap_or(u64::MAX);
    let stored_rights = entry_opt.map(|e| e.rights).unwrap_or(Rights::empty());
    // Pas d'early return : on fait toujours les deux comparaisons
    let gen_ok    = stored_gen == token.generation;
    let rights_ok = stored_rights.contains(rights);
    if !gen_ok || !rights_ok || entry_opt.is_none() {
        return Err(CapError::Denied);  // Toujours Denied — jamais NotFound
    }
    Ok(())
}

2.3 — Rights bitflags (security/capability/rights.rs)
Constante	Bit	Utilisation	Module concerné
READ	1 << 0	Lecture d'un L-Obj	fs/exofs/syscall/object_read.rs
WRITE	1 << 1	Écriture d'un L-Obj	fs/exofs/syscall/object_write.rs
EXEC	1 << 2	exec() sur ObjectKind::Code	process/exec.rs
DELETE	1 << 3	Suppression d'un L-Obj	fs/exofs/syscall/object_delete.rs
CREATE_CHILD	1 << 4	Création d'enfant dans PathIndex	fs/exofs/syscall/object_create.rs
STAT	1 << 5	Lecture metadata (ObjectMeta)	fs/exofs/syscall/object_stat.rs
SET_META	1 << 6	Modification metadata	fs/exofs/syscall/object_set_meta.rs
DELEGATE	1 << 7	Délégation de sous-ensemble de droits	security/capability/
SNAPSHOT_ACCESS	1 << 8	Accès à un snapshot en lecture	fs/exofs/syscall/snapshot_mount.rs
IPC_SEND	1 << 9	Envoi message sur un channel	ipc/channel/
INSPECT_CONTENT	1 << 10	SYS_EXOFS_GET_CONTENT_HASH — audité	fs/exofs/syscall/get_content_hash.rs
SNAPSHOT_CREATE	1 << 11	Création snapshot	fs/exofs/syscall/snapshot_create.rs
RELATION_CREATE	1 << 12	Création relation typée	fs/exofs/syscall/relation_create.rs
GC_TRIGGER	1 << 13	Déclenchement GC manuel	fs/exofs/syscall/gc_trigger.rs

2.4 — ObjectKind et règles d'accès
ObjectKind	Valeur	Restriction principale	Règle
Blob	0	Aucune restriction spécifique	LOBJ-01 : accès via verify_cap normal
Code	1	exec() uniquement avec Rights::EXEC	EXEC-02 : vérifier kind avant chargement ELF
Config	2	Validation schéma obligatoire	LOBJ-01 : read/write avec rights normaux
Secret	3	BlobId JAMAIS exposé même avec INSPECT_CONTENT	SEC-07 CRITIQUE
PathIndex	4	Toujours Class2 (mutable)	PATH-01 : SipHash-keyed obligatoire
Relation	5	Lien typé — droits hérités du parent	LOBJ-01 extension

🔴 LOBJ-SECRET — ObjectKind::Secret : le BlobId (hash du contenu) ne doit JAMAIS être retourné à l'utilisateur, même avec Rights::INSPECT_CONTENT. Le BlobId permettrait de savoir si deux secrets ont le même contenu.
🔴 LOBJ-EXEC — INTERDIT : exec() sur ObjectKind::Secret. Le binaire serait déchiffré et exécuté sans contrôle de signature.

3 — Sécurité au Boot : gap APs (Z-AI CVE-EXO-001)
Entre l'initialisation de security::capability (step 17) et security::access_control::checker (step 18), les APs SMP pourraient tenter des IPC sans vérification active. Cette fenêtre est exploitable.

🔴 BOOT-SEC — Flag atomique SECURITY_READY requis. Les APs spin-wait sur ce flag avant toute IPC. Stocké dans security/mod.rs.

// security/mod.rs
pub static SECURITY_READY: AtomicBool = AtomicBool::new(false);
 
// arch/x86_64/smp/init.rs — chaque AP attend avant toute IPC
pub fn ap_init() {
    // ... GDT, IDT, TSS setup ...
    while !security::SECURITY_READY.load(Ordering::Acquire) {
        core::hint::spin_loop();  // spin-wait — sûr car court
    }
    // Désormais sûr de faire de l'IPC et d'accéder aux objets ExoFS
    scheduler::register_cpu();
}
 
// Séquence boot BSP (dans kmain) :
// Step 17 : security::capability::init()
// Step 18 : security::access_control::checker::init()
//           security::crypto::rng::init()  ← graine pour SipHash FUTEX
//           security::SECURITY_READY.store(true, Ordering::Release);  ← ICI
// Step 26 : arch::x86_64::smp::start_aps()  ← APs démarrés APRÈS flag

4 — Cryptographie dans fs/exofs/crypto/
4.1 — Pipeline obligatoire
🔴 CRYPTO-PIPE — Ordre inviolable : données brutes → Blake3(BlobId) → compression → XChaCha20-Poly1305 → disque
🔴 CRYPTO-ORDER — INTERDIT : compresser après chiffrement. Le ciphertext est incompressible (entropie maximale). Blake3 TOUJOURS sur données brutes non-compressées.
🔴 CRYPTO-NONCE — INTERDIT : réutiliser un nonce avec la même clé XChaCha20 — violation cryptographique totale. Chaque objet a son propre nonce unique.
✅ CRYPTO-SHRED — Crypto-shredding : oublier l'ObjectKey = suppression sécurisée sans effacement physique des blobs.

// Pipeline lecture/écriture d'un L-Obj Secret :
 
// ÉCRITURE (secret_writer.rs) :
// 1. Calculer BlobId = Blake3(données brutes)          ← AVANT compression
// 2. Comprimer les données brutes (LZ4 ou Zstd)
// 3. Dériver ObjectKey = HKDF(volume_key, object_id)   ← unique par objet
// 4. Générer nonce (24 bytes) = RDRAND + compteur atomique par volume
// 5. Chiffrer : XChaCha20-Poly1305(ObjectKey, nonce, données_comprimées)
// 6. Écrire [nonce | ciphertext | Poly1305 tag] sur disque
 
// LECTURE (secret_reader.rs) :
// 1. Lire [nonce | ciphertext | tag] depuis disque
// 2. Dériver ObjectKey = HKDF(volume_key, object_id)
// 3. Déchiffrer et vérifier tag Poly1305
// 4. Décompresser
// 5. Vérifier BlobId = Blake3(résultat) == attendu     ← intégrité

4.2 — Gestion des clés (key_derivation.rs, key_storage.rs)
// Hiérarchie des clés :
MasterKey (256 bits)
    │ sealed TPM (priorité) OU chiffré par KDF(PIN + sel)
    │ stocké : key_storage.rs
    ▼
VolumeKey = HKDF(MasterKey, volume_uuid)   // une par volume monté
    │ dérivée à chaque montage
    │ stockée en mémoire kernel uniquement (jamais sur disque)
    ▼
ObjectKey = HKDF(VolumeKey, object_id)     // une par L-Obj Secret
    │ dérivée à la demande lors du chiffrement/déchiffrement
    │ NOT cachée — recalculée à chaque accès
    ▼
Crypto-shredding : effacer VolumeKey → tous les ObjectKey inaccessibles
✅ KEY-HKDF — HKDF utilisé pour toutes les dérivations de clés. La clé parent ne peut pas être retrouvée depuis une clé dérivée.
⚠️ KEY-TPM — MasterKey scellée en TPM en priorité. Si TPM absent : chiffrement par KDF(Argon2, PIN + sel aléatoire 32B). L'algo KDF et le sel DOIVENT être documentés dans key_storage.rs.
✅ KEY-ROTATE — Rotation VolumeKey possible sans rechiffrement des données (key_rotation.rs). Les blobs existants restent déchiffrables via l'ancienne clé pendant la transition.

4.3 — Nonces XChaCha20 — mécanisme correct
🐛 LACUNE CRYPTO-NONCE-RDRAND — entropy.rs spécifie 'RDRAND + TSC pour nonces'. RDRAND seul est insuffisant : si RDRAND est absent ou faible, tous les nonces se ressemblent. Le TSC n'est pas un bon générateur de nonces non-prévisibles.

// ❌ FAUTIF — RDRAND + TSC seul = insuffisant si RDRAND défaillant
fn generate_nonce() -> [u8; 24] {
    let rdrand = arch::rdrand64();
    let tsc    = arch::rdtsc();
    // Combinaison déterministe → prévisible si RDRAND faiblit
    let mut n = [0u8; 24];
    n[0..8].copy_from_slice(&rdrand.to_le_bytes());
    n[8..16].copy_from_slice(&tsc.to_le_bytes());
    n   // 8 bytes de vraie entropie seulement
}
 
// ✅ CORRECT — compteur atomique + RDRAND comme sel
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);
 
fn generate_nonce(volume_key: &VolumeKey, object_id: ObjectId) -> [u8; 24] {
    // Compteur global monotone — garantit l'unicité même sans RDRAND
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    // XChaCha20 a un nonce 192 bits = 24 bytes
    // Dériver le nonce depuis : counter + object_id + RDRAND (comme diversifiant)
    let rdrand  = arch::rdrand64().unwrap_or(0);
    let mut ctx = blake3::Hasher::new();
    ctx.update(&counter.to_le_bytes());
    ctx.update(object_id.as_bytes());
    ctx.update(&rdrand.to_le_bytes());
    let mut nonce = [0u8; 24];
    ctx.finalize_xof().fill(&mut nonce);
    nonce
}

4.4 — Source des implémentations (trou critique)
🐛 LACUNE CRYPTO-SOURCE — ExoFS v3 décrit xchacha20.rs, blake3.rs, key_derivation.rs (HKDF), master_key.rs (Argon2) mais NE SPÉCIFIE JAMAIS leur source. Aucun Cargo.toml, aucune mention de crate. Un générateur de code peut implémenter ces algos from scratch, ce qui est dangereux.

🔴 CRYPTO-CRATES — Toutes les primitives crypto DOIVENT utiliser les crates RustCrypto no_std validées. Ces crates sont compatibles Ring0 (no_std, no alloc ou alloc contrôlé) et ont des vecteurs de test IETF.

Usage	Crate RustCrypto	Feature no_std	Vecteurs IETF
Blake3 checksums + BlobId	blake3	default-features = false	Oui
XChaCha20-Poly1305	chacha20poly1305	default-features = false, features=["xchacha20"]	Oui — RFC 8439
HKDF (dérivation clés)	hkdf	default-features = false	Oui — RFC 5869
Argon2 (KDF PIN)	argon2	default-features = false	Oui — RFC 9106
SipHash (PathIndex)	siphasher	default-features = false	N/A — keyed hash
LZ4 compression	lz4_flex	default-features = false	N/A
Zstd compression	zstd-sys	kernel mode bindings	N/A

# Cargo.toml (kernel) — section [dependencies] à ajouter
blake3          = { version = "1",    default-features = false }
chacha20poly1305= { version = "0.10", default-features = false, features = ["xchacha20"] }
hkdf            = { version = "0.12", default-features = false }
argon2          = { version = "0.5",  default-features = false }
siphasher        = { version = "0.3",  default-features = false }
lz4_flex        = { version = "0.11", default-features = false }
# INTERDIT : libsodium (requiert std + libc)
# INTERDIT : ring (trop de dépendances système)
# INTERDIT : implémentation from scratch (risque cryptographique)

5 — Système d'Audit (fs/exofs/audit/)
5.1 — Structure
fs/exofs/audit/
├── mod.rs           # Audit API : audit_log!(op, actor_cap, object_id, result)
├── audit_entry.rs   # struct AuditEntry { ts: u64, op: AuditOp, actor_cap: CapToken,
│                    #   object_id: ObjectId, result: AuditResult }
├── ring_buffer.rs   # Ring buffer lock-free pour les entrées
│                    # Taille fixe — anciens logs écrasés (pas de blocage)
├── audit_reader.rs  # Lecture entrées audit via syscall userspace
└── audit_ops.rs     # enum AuditOp : Read, Write, Create, Delete, Exec, CapVerify,
                     #               SnapCreate, ContentHash, GcTrigger, EpochCommit

✅ AUDIT-01 — Toute opération sur un L-Obj doit produire une entrée audit (succès et échec). L'audit des échecs est plus important que celui des succès pour détecter les attaques.
🔴 AUDIT-02 — SYS_EXOFS_GET_CONTENT_HASH est une opération sensible (révèle si deux processus ont des fichiers identiques). Toujours auditée, Rights::INSPECT_CONTENT requis.
✅ AUDIT-03 — Ring buffer audit = lock-free. Si plein, les plus anciennes entrées sont écrasées. Ne jamais bloquer le hot path pour l'audit.
⚠️ AUDIT-04 — Nom de la constante : l'audit ring buffer référencé dans ExoFS v3 comme 'SEC-09' est distinct du CI grep 'SEC-09' du doc kernel. Renommer en AUDIT-RING pour éviter la confusion.

6 — Sécurité du PathIndex (hash anti-DoS)
🔴 PATH-01 — PathIndex utilise SipHash-2-4 avec une clé secrète mount_secret_key:[u8;16] aléatoire au montage. Sans clé secrète, un attaquant peut construire des chemins qui hashent tous sur le même bucket → O(n) lookup → DoS.
🔴 PATH-02 — INTERDIT : hash non-keyed pour PathIndex — SipHash sans clé = vulnérable HashDoS
✅ PATH-03 — Collision de hash → comparaison du nom COMPLET byte-à-byte. Ne jamais se fier au hash seul.

🐛 LACUNE PATH-MOUNT-KEY — La mount_secret_key:[u8;16] doit être initialisée depuis security::crypto::rng au moment du montage. ExoFS v3 ne précise ni où ni comment cette clé est générée. Sans cette précision, un générateur de code peut utiliser une clé fixe (tous zéros) = vulnérabilité HashDoS.

// fs/exofs/path/mod.rs — initialisation au montage
pub fn mount(dev: &Device, flags: MountFlags) -> Result<ExofsMount, ExofsError> {
    // ...
    // Générer la clé secrète de hash DEPUIS le RNG kernel
    // Ne jamais utiliser de clé fixe ou de constante
    let mut mount_secret_key = [0u8; 16];
    security::crypto::rng::fill_random(&mut mount_secret_key)?;
    // SECURITY_READY doit être true avant ce point (step 18)
    let path_index = PathIndex::new(mount_secret_key);
    // ...
}
 
// path/path_index.rs — utilisation
pub fn hash_name(key: &[u8; 16], name: &[u8]) -> u64 {
    let mut hasher = SipHasher24::new_with_key(key);
    hasher.write(name);
    hasher.finish()
}

7 — Quotas Capability-Bound (fs/exofs/quota/)
Les quotas ExoFS sont liés aux capabilities, pas aux UIDs. C'est une différence fondamentale avec ext4.

🔴 QUOTA-01 — ENOSPC retourné si quota dépassé — vérifié AVANT toute allocation physique.
✅ QUOTA-02 — Quota par capability (pas par UID/GID). Un processus peut avoir plusieurs quotas différents selon ses capabilities.
✅ QUOTA-03 — Usage tracking par capability — quota_tracker.rs incrémente/décrémente atomiquement.
✅ QUOTA-04 — verify_cap() avec Rights::WRITE vérifié AVANT la vérification quota — l'ordre est important (fail fast sur droits).

8 — Lacunes et Incohérences identifiées
8.1 — Lacunes critiques sécurité
🐛 LACUNE LAC-01 — CRITIQUE — verify() non constant-time — Les règles ExoFS v3 (SEC-01 à SEC-08) n'imposent pas explicitement que verify() soit constant-time. Un attaquant peut mesurer la différence de temps entre un token révoqué (objet existe, generation ≠) et un token inexistant (objet absent) pour déduire quels ObjectIds sont valides. Règle CAP-05 à ajouter obligatoirement.

🐛 LACUNE LAC-02 — SEC-08 référence preuve Coq obsolète — SEC-08 : 'Délégation capability : droits_délégués ⊆ droits_délégateur — PROP-3 prouvée Coq'. La preuve Coq est supprimée en v6. La règle reste valide mais sa garantie n'est plus prouvée formellement. Remplacer par : vérifiée par proptest + INVARIANTS.md + CI algebraic check.

🐛 LACUNE LAC-03 — CRITIQUE — Crates crypto non spécifiées — Voir Section 4.4. xchacha20.rs, blake3.rs, HKDF, Argon2 existent dans l'arborescence mais leur source n'est pas définie. Sans Cargo.toml explicite, tout générateur de code peut produire une implémentation from scratch non testée.

🐛 LACUNE LAC-04 — CRITIQUE — Nonce RDRAND+TSC insuffisant — Voir Section 4.3. entropy.rs ne garantit pas l'unicité des nonces si RDRAND est absent ou faible. Corriger avec compteur atomique global + HKDF(nonce_base, counter, object_id).

🐛 LACUNE LAC-05 — mount_secret_key non documentée — PathIndex utilise SipHash-keyed mais la source et le mécanisme de génération de mount_secret_key ne sont jamais décrits. Risque : clé nulle ou fixe.

🐛 LACUNE LAC-06 — key_storage.rs 'chiffré PIN' sans algo — Si TPM absent, la clé est 'chiffrée PIN'. Ni l'algorithme KDF (Argon2 ?), ni les paramètres (time_cost, memory_cost, parallelism), ni le format de stockage (superblock ? partition séparée ?) ne sont documentés.

🐛 LACUNE LAC-07 — Collision nomenclature SEC-09 — ExoFS v3 référence 'ring buffer SEC-09' et le doc kernel DOC1-10 définit SEC-09 comme 'CI grep anti-bypass'. Ce sont deux choses différentes avec le même identifiant. Renommer l'audit ring buffer ExoFS en AUDIT-RING-SEC.

🐛 LACUNE LAC-08 — PROC-03 signal pendant exec non corrigé dans do_exec() — La table PROC-03 identifie le bug (signal livré entre load ELF et reset TCB → handler vers ancien processus) mais les étapes de do_exec() n'incluent pas le blocage des signaux. À ajouter comme étape 3 explicite.

8.2 — Tableau de priorité des corrections
ID	Priorité	Impact	Correction requise
LAC-01	P0	Timing side-channel ObjectId enumeration	Implémenter verify() constant-time (Section 2.2)
LAC-03	P0	Crypto non-testée → vulnérabilités	Ajouter Cargo.toml avec crates RustCrypto (Section 4.4)
LAC-04	P0	Réutilisation nonce XChaCha20	Compteur atomique + dérivation HKDF (Section 4.3)
LAC-06	P0	Clé maître sans algo → stockage insécurisé	Documenter Argon2 + format key_storage.rs
LAC-05	P1	DoS PathIndex si clé nulle	Documenter init depuis rng::fill_random() au montage
LAC-08	P1	Signal handler exploit inter-exec	Ajouter étape 3 block_all dans do_exec()
LAC-02	P2	Garantie formelle manquante	Proptest + INVARIANTS.md pour délégation caps
LAC-07	P2	Confusion dans les références	Renommer audit ring buffer en AUDIT-RING-SEC

9 — Checklist Sécurité avant commit
#	Vérification	Modules
S-01	verify_cap() appelé AVANT tout accès objet ExoFS — sans exception	fs/exofs/syscall/*
S-02	verify() = constant-time — pas d'early return différentiel	security/capability/verify.rs
S-03	security::access_control::check_access() appelé directement (pas capability_bridge)	ipc/, fs/exofs/
S-04	SECURITY_READY flag atomique — APs spin-wait avant toute IPC	security/mod.rs, arch/smp/
S-05	Blake3 calculé sur données brutes AVANT compression (jamais après)	fs/exofs/crypto/blake3.rs
S-06	XChaCha20 : nonce = compteur atomique + HKDF(object_id) — jamais RDRAND seul	fs/exofs/crypto/xchacha20.rs
S-07	Pipeline : données → Blake3 → compression → XChaCha20 → disque	fs/exofs/crypto/secret_writer.rs
S-08	Cargo.toml liste blake3, chacha20poly1305, hkdf, argon2 no_std	kernel/Cargo.toml
S-09	ObjectKind::Secret : BlobId jamais retourné même avec INSPECT_CONTENT	fs/exofs/syscall/get_content_hash.rs
S-10	exec() sur ObjectKind::Secret = Err(NotExecutable)	process/exec.rs
S-11	mount_secret_key initialisée depuis security::crypto::rng au montage	fs/exofs/path/mod.rs
S-12	PathIndex hash = SipHash-2-4 keyed — jamais hash non-keyed	fs/exofs/path/path_index.rs
S-13	Quota vérifié AVANT allocation — ENOSPC immédiat si dépassé	fs/exofs/quota/quota_enforcement.rs
S-14	Audit ring buffer : toutes opérations loggées succès ET échec	fs/exofs/audit/ring_buffer.rs
S-15	SYS_EXOFS_GET_CONTENT_HASH toujours auditée (SEC-06)	fs/exofs/syscall/get_content_hash.rs
S-16	key_storage.rs : si no-TPM → Argon2 + sel 32B + paramètres documentés	fs/exofs/crypto/key_storage.rs
S-17	do_exec() étape 3 : block_all_except_kill() avant chargement ELF	process/exec.rs
S-18	Cap table fork : dup_for_fork() transactionnel — rollback si échec	process/fork.rs
S-19	do_exit() : cap_table.revoke_all() sans exception	process/exit.rs
S-20	SEC-08 texte : remplacer 'PROP-3 prouvée Coq' par proptest + INVARIANTS.md	security/capability/
S-21	CI grep : aucun import direct de security::capability::table sans passer par verify()	CI / Makefile
S-22	VMA SignalTcb : VM_DONTCOPY | VM_DONTEXPAND dès exec()	process/lifecycle/exec.rs

