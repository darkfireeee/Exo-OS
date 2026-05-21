# ExoOS v0.2.0 — Audit Critique (P0)
## Bugs Bloquants Sécurité & Intégrité des Données

**Auteur :** claude-beta  
**Date :** 2026-05-20  
**Sévérité :** P0 — À corriger AVANT tout autre travail v0.2.0  
**Checklist :** BLOC -1 (B-01, B-09) + CVE-2012-0217

---

## BUG-P0-01 — VirtIO BAR hardcodé (CORR-86 / B-01)

**Fichier :** `kernel/src/fs/exofs/storage/virtio_adapter.rs`  
**Lignes :** 10, 72–74  
**Checklist :** B-01

### Code fautif

```rust
// kernel/src/fs/exofs/storage/virtio_adapter.rs — lignes 10, 72-74

pub const DEFAULT_VIRTIO_BLK_MMIO_BASE: usize = 0x1000_0000;  // ← HARDCODÉ

pub fn init_global_disk() {
    init_global_disk_with_mmio(
        DEFAULT_VIRTIO_BLK_MMIO_BASE,   // ← jamais lu depuis PCI config space
        ...
    )
}
```

### Impact

L'adresse MMIO du disque VirtIO est hardcodée à `0x10000000`. Cette adresse
n'est valide que dans une configuration QEMU spécifique. Sur tout autre
firmware (UEFI, BIOS avec configuration PCI différente), la BAR0 du device
VirtIO-Blk peut se trouver à une adresse différente. Conséquence : ExoFS
ne peut ni lire ni écrire sur disque → **B-02 (persistance ExoFS) dépend
directement de B-01**.

### Correction requise

```rust
// Nouveau : lire BAR0 depuis le PCI config space
pub fn init_global_disk() {
    // Trouver le device VirtIO-Blk (vendor=0x1AF4, device=0x1001/0x1042)
    let bar0 = pci::find_virtio_blk_bar0()
        .expect("virtio-blk: aucun device trouvé dans le bus PCI");

    // Décoder BAR0 : masquer bits de type, aligner sur 4 KiB
    let mmio_base = (bar0 & !0xFFF) as usize;

    init_global_disk_with_mmio(mmio_base, capacity_from_device());
}
```

### Test de validation (B-01)

```
Log attendu au boot : "virtio-blk: BAR0 MMIO base = 0x<addr> (≠ 0x10000000)"
```

---

## BUG-P0-02 — write_blob() ignore le flag immutable (S-19 / CORR-84)

**Fichier :** `kernel/src/fs/exofs/syscall/object_write.rs`  
**Lignes :** 103–133  
**Checklist :** S-19, S-20

### Code fautif

```rust
// kernel/src/fs/exofs/syscall/object_write.rs — fn write_blob()

fn write_blob(blob_id: BlobId, offset: u64, data: &[u8]) -> ExofsResult<WriteResult> {
    // ... validation d'offset ...

    ensure_blob_cached(blob_id)?;

    // ← MANQUANT : vérification is_immutable() avant toute écriture
    // Si le blob est immuable (ExoLedger zone P0), l'écriture doit être
    // refusée avec ExofsError::PermissionDenied ET auditée dans ExoLedger.

    BLOB_CACHE.write_at(blob_id, start, data)?;   // ← écrit sans vérifier
    persist_cached_blob_if_disk(blob_id)?;
    ...
}
```

### Infrastructure existante mais non utilisée

```rust
// kernel/src/fs/exofs/objects/object_meta.rs — déjà implémenté

pub const META_FLAG_IMMUTABLE: u32 = 1 << 0;

impl ObjectMeta {
    pub fn is_immutable(&self) -> bool {
        self.extra_flags & META_FLAG_IMMUTABLE != 0
    }
}

// kernel/src/fs/exofs/objects/logical_object.rs

impl LogicalObject {
    pub fn is_immutable(&self) -> bool {
        self.meta.is_immutable()
    }
}
```

### Impact

Tout blob marqué `META_FLAG_IMMUTABLE` (notamment la **zone P0 d'ExoLedger**)
peut être modifié par n'importe quel appelant syscall. La chaîne d'audit
Blake3 peut être corrompue silencieusement. Ceci invalide les garanties
de S-19, S-20 et l'invariant ExoLedger complet.

### Correction requise

```rust
fn write_blob(blob_id: BlobId, offset: u64, data: &[u8]) -> ExofsResult<WriteResult> {
    // ... validation d'offset ...

    ensure_blob_cached(blob_id)?;

    // CORRECTION S-19 : vérification immutabilité AVANT toute modification
    if let Some(obj) = object_store::get_object(blob_id) {
        if obj.is_immutable() {
            // Auditer la tentative dans ExoLedger (CORR-84)
            crate::security::exoledger::audit_immutable_write_attempt(blob_id);
            return Err(ExofsError::PermissionDenied);
        }
    }

    BLOB_CACHE.write_at(blob_id, start, data)?;
    persist_cached_blob_if_disk(blob_id)?;
    ...
}
```

### Test de validation (S-20)

```rust
// Test requis par CORR-84
#[test]
fn test_write_on_immutable_blob_denied_and_audited() {
    let blob_id = create_blob_and_mark_immutable();
    let result = write_blob(blob_id, 0, b"tamper");
    assert_eq!(result, Err(ExofsError::PermissionDenied));
    // Vérifier que ExoLedger contient l'entrée d'audit
    assert!(exoledger::last_audit_entry_is_immutable_violation(blob_id));
}
```

---

## BUG-P0-03 — CVE-2012-0217 : SYSRETQ avec RSP=0 insuffisant

**Fichier :** `kernel/src/arch/x86_64/syscall.rs`  
**Lignes :** 379–396 (vérification), 273 (sysretq)  
**Checklist :** (non listé — bug de sécurité latent non couvert par la checklist actuelle)

### Description du problème

Le code actuel détecte un RSP ou RCX non-canonique et le force à 0 :

```rust
// kernel/src/arch/x86_64/syscall.rs — post-dispatch

if !is_user_return_addr(frame.rcx) {
    frame.rcx = 0;   // ← RIP retour = 0
}
if !is_user_return_addr(frame.rsp) {
    frame.rsp = 0;   // ← RSP = 0
}
```

Puis dans l'ASM, `sysretq` est exécuté inconditionnellement :

```asm
mov   rsp, qword ptr gs:[0x08]   ; charge RSP depuis slot per-CPU
swapgs
sysretq                          ; ← TOUJOURS SYSRETQ, jamais IRETQ
```

### Pourquoi RSP=0 ne protège pas

L'instruction `sysretq` charge d'abord **RCX → RIP**, puis **le processeur
valide la canonicité de RIP**. Si RCX = 0, l'adresse est canonique (zone
utilisateur basse) et le processeur continue en Ring 3 sans fault.

Le vrai vecteur de CVE-2012-0217 est le cas où **RSP est non-canonique**
au moment où `sysretq` s'exécute avec **RCX non-canonique** : le `#GP`
se déclenche avec le CPL encore à 0 (la transition Ring0→Ring3 n'est pas
atomique), exposant une fenêtre d'escalade de privilège.

Forcer `frame.rsp = 0` puis le charger dans RSP avant `sysretq` ne suffit
pas : si `frame.rcx` (RIP) est non-canonique malgré la mise à 0 (condition
logique incorrecte), ou si l'attaquant manipule la frame après la vérification,
la fenêtre subsiste.

### Mitigation correcte

```asm
; Dans syscall_entry_asm, juste avant sysretq :
;
; Vérifier canonicité RCX (RIP retour) et RSP — si non-canonique → IRETQ
;
; Test canonicité RCX : bit 47 doit être sign-extended vers les bits 48..63
mov rax, rcx
sar rax, 47
test rax, rax
jnz use_iretq          ; non-canonique → fallback IRETQ

; Test canonicité RSP
mov rax, qword ptr gs:[0x08]
sar rax, 47
test rax, rax
jnz use_iretq

; Chemin normal SYSRETQ
mov   rsp, qword ptr gs:[0x08]
swapgs
sysretq

use_iretq:
; Construire une frame IRETQ sur la pile kernel
; [RIP, CS=0x33, RFLAGS=r11, RSP, SS=0x2B]
; puis : iretq
```

### Correction en Rust (côté post-dispatch)

```rust
// Marquer dans la frame si IRETQ est requis
if !is_user_canonical(frame.rcx) || !is_user_canonical(frame.rsp) {
    frame.rcx = 0;
    frame.rsp = USER_STACK_TOP.as_u64(); // stack valide → SIGSEGV en Ring3
    // Signaler à l'ASM de prendre le chemin IRETQ
    frame.use_iretq = true;
}
```

---

## INC-P0-04 — IPC_MAX_SMALL_MSG orpheline et trompeuse

**Fichier :** `kernel/src/memory/core/constants.rs`  
**Ligne :** 152  
**Checklist :** (cohérence entre modules)

### Code fautif

```rust
// kernel/src/memory/core/constants.rs

/// Taille maximale d'un message IPC petit (inline dans le ring)
pub const IPC_MAX_SMALL_MSG: usize = 4080;   // ← JAMAIS utilisée
```

```rust
// kernel/src/ipc/core/constants.rs

pub const MAX_MSG_SIZE: usize = 240;          // ← valeur réelle du système
```

### Impact

La constante `IPC_MAX_SMALL_MSG = 4080` n'est référencée nulle part dans
le code (confirmé par grep complet). Elle est définie dans le module
`memory/core/constants.rs` à une valeur 17× supérieure à la vraie
limite IPC (`MAX_MSG_SIZE = 240`).

Tout développeur futur lisant `memory/core/constants.rs` pourrait croire
que les messages IPC peuvent faire jusqu'à 4080 octets, ce qui est faux.
Des buffers surdimensionnés, des validations incorrectes, ou des
comportements indéfinis pourraient en résulter.

### Correction requise

```rust
// Supprimer IPC_MAX_SMALL_MSG de memory/core/constants.rs

// Remplacer par une référence explicite :
// (dans tout code qui en aurait besoin)
use crate::ipc::core::constants::MAX_MSG_SIZE;
```

---

## Récapitulatif P0

| ID | Fichier | Ligne | Gravité | Checklist |
|---|---|---|---|---|
| BUG-P0-01 | `kernel/src/fs/exofs/storage/virtio_adapter.rs` | 10, 72–74 | Critique — pas de persistance disque | B-01, B-02 |
| BUG-P0-02 | `kernel/src/fs/exofs/syscall/object_write.rs` | 103–133 | Critique — bypass immutabilité ExoLedger | S-19, S-20 |
| BUG-P0-03 | `kernel/src/arch/x86_64/syscall.rs` | 273, 379–396 | Critique — CVE-2012-0217 non mitigé | (hors checklist) |
| INC-P0-04 | `kernel/src/memory/core/constants.rs` | 152 | Moyenne — constante orpheline trompeuse | (cohérence) |

---

*claude-beta — ExoOS v0.2.0 Audit — AUDIT-CRITIQUE.md*
