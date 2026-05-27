# SPEC-EXO-DISPLAY-PROTOCOL-STRATA — Format Natif ExoOS
## Zéro rwx · Zéro uid:gid · Format Capability Partout

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace SPEC-EXO-DISPLAY-PROTOCOL.md

---

## 1. Le Problème

Linux affiche les permissions sous forme POSIX :
```
drwxr-xr-x  2 eric users 4096  mai 26  documents/
-rw-r--r--  1 eric users  124  mai 14  rapport.txt
```

ExoOS n'a ni `rwx` traditionnel, ni `uid`, ni `gid`, ni inode numéro.
Utiliser ce format serait un **mensonge architectural** — il cacherait le modèle réel derrière une façade inexistante.

**Règle absolue Strata :** Aucun outil système ExoOS natif n'affiche jamais `rwx` ni `uid:gid`.

*(musl-exo peut générer ce format pour les apps POSIX qui en ont besoin — c'est la couche compat.)*

---

## 2. Modèle Mental ExoOS

Tout objet dans ExoOS a :

| Attribut | Type | Description |
|---|---|---|
| **Type** | `ObjectKind` | blob / dir / relation / secret / code / config |
| **Capabilities** | `RightsMask` | read / write / exec / list / link / seal / derive |
| **Token** | `CapToken` | identifiant de possession (u64 opaque) |
| **Epoch** | `EpochId` | version dans ExoFS (monotone croissant) |
| **Flags** | `ObjectFlags` | sealed / signed / encrypted / pinned / snapshot / ghost |
| **Taille** | `u64` | octets (blob) ou nb d'entrées (dir) |
| **Hash** | `ContentHash` | BLAKE3 du contenu (content-addressed) |
| **Nom** | `PathComponent` | composant de chemin UTF-8 |

---

## 3. Format Long — `exo ls -l`

### 3.1 Syntaxe Complète

```
<type> <rights> <flags>   <token>  <epoch>    <size>       <hash-short>  <nom>
```

### 3.2 Caractères de Type

| Char | Signification |
|---|---|
| `d` | Répertoire (PathIndex dans ExoFS) |
| `b` | Blob de données (fichier ordinaire) |
| `r` | Relation typée (lien sémantique entre objets) |
| `s` | Secret (contenu chiffré, jamais lisible directement) |
| `x` | Code exécutable (blob + flag exec) |
| `c` | Configuration (blob + format structuré connu) |
| `?` | Type inconnu ou non résolu |

### 3.3 Masque de Droits — 7 bits

Format : `rwxlksd` — chaque position est la lettre ou `-` si absent.

| Pos | Lettre | Signification |
|---|---|---|
| 1 | `r` | **read** — lire le contenu |
| 2 | `w` | **write** — modifier |
| 3 | `x` | **exec** — exécuter (code) / traverser (dir) |
| 4 | `l` | **list** — lister les entrées |
| 5 | `k` | **link** — créer des relations vers cet objet |
| 6 | `s` | **seal** — rendre immutable |
| 7 | `d` | **derive** — dériver des sous-capabilities |

### 3.4 Flags d'État

| Code | Signification |
|---|---|
| `[sealed]` | Immuable depuis l'epoch indiquée |
| `[✓sig]` | Signature Ed25519 vérifiée |
| `[enc]` | Contenu chiffré (XChaCha20 par objet) |
| `[pin]` | Épinglé — non éligible au GC |
| `[snap]` | Snapshot — lecture seule depuis une epoch |
| `[ghost]` | Supprimé logiquement, en attente de GC |
| `·` | Aucun flag particulier |
| `[usb]` | Objet sur volume USB monté |
| `[scanned ✓]` | Scanné par ExoShield, propre |
| `[scanned ⚠]` | Menace détectée — transfert restreint |

### 3.5 Token

Format : `@` + 4 derniers octets du CapToken en hex.
Token complet visible avec `exo stat <obj>`.

---

## 4. Exemples Concrets

### 4.1 Répertoire Home

```
$ exo ls -l /home/eric/

d  rwxl---  ·         @9f3c  ep:0042  4 entries     --------  documents/
b  rw-----  [enc]     @7a1e  ep:0041  124.3 KiB     a3f2b1c8  rapport.exo
x  r-x----  [✓sig]    @3d8f  ep:0038  2.1 MiB       9e4a72f1  shell
s  r------  [sealed]  @2b9d  ep:0039  256 B         --------  keyring.secret
r  r------  ·         @1f7b  ep:0040  → documents/  --------  docs -> documents/
```

### 4.2 Répertoire Apps

```
$ exo ls -l /apps/

d  rwxl---  ·         @0001  ep:0010  3 entries   --------  exo-calendar/
d  rwxl---  ·         @0002  ep:0011  2 entries   --------  exo-texteditor/
```

```
$ exo ls -l /apps/exo-calendar/

x  r-x----  [✓sig]    @3d8f  ep:0038  2.1 MiB     9e4a72f1  bin/exo-calendar
c  rw-----  ·         @2b9d  ep:0041  4.2 KiB     bc38f091  config.exo
d  rwxl---  ·         @9f3c  ep:0042  12 entries  --------  data/
```

### 4.3 Clé USB Montée

```
$ exo ls -l /mnt/usb/

d  ----l--  [usb]            @0000  ep:----  4 entries   --------  docs/
x  r-x----  [usb][scanned ✓] @0000  ep:----  2.1 MiB     9e4a72f1  app.elf
b  rw-----  [usb]            @0000  ep:----  450 KiB     bc38f091  data.db
b  rw-----  [usb]            @0000  ep:----  12 KiB      --------  notes.txt
```

### 4.4 Serveurs Ring1

```
$ exo ls -l /srv/

x  r-x----  [sealed][✓sig]  @0010  ep:0001  1.2 MiB  e3a1f092  vfs_server
x  r-x----  [sealed][✓sig]  @0011  ep:0001  980 KiB  74bc8a21  crypto_server
x  r-x----  [sealed][✓sig]  @0017  ep:0001  3.4 MiB  91d2c4f8  exo_shield
x  r-x----  [sealed][✓sig]  @0018  ep:0001  1.8 MiB  2e5a9b37  exosh
```

---

## 5. Format Court — `exo ls`

Sans `-l`, format condensé sur une ligne :

```
$ exo ls /home/eric/

documents/     [d  rwxl]  4 entries
rapport.exo    [b  rw   enc]  124 KiB
shell          [x  r-x  ✓]   2.1 MiB
keyring.secret [s  r-   sealed]
```

---

## 6. `exo stat <obj>` — Détail Complet

```
$ exo stat /home/eric/shell

Object : /home/eric/shell
─────────────────────────────────────────────────────────
Type         : code (exécutable ELF)
Rights       : r-x----  (read + exec, pas de write)
Flags        : [✓sig]
Token        : @3d8f → CapToken(0x9f3c7a1e_3d8f2b1a)
Epoch        : 0038
Size         : 2,204,672 bytes (2.1 MiB)
Content hash : 9e4a72f1c3d8b4a2e7f90123456789012345678901234567890123456789012
Signed by    : ExoOS Dev Key (Ed25519)
Created      : epoch 0001  (2026-04-12 14:23:41)
Modified     : epoch 0038  (2026-05-14 09:17:22)
Relations    : 0 inbound, 0 outbound
Snapshots    : 3 disponibles (epochs 0001, 0020, 0038)
ExoLedger    : dernière écriture seq:4187
```

---

## 7. `exo cap list --pid <pid>` — Capabilities d'un Processus

```
$ exo cap list --pid 42

Processus : exosh  (PID 42)
─────────────────────────────────────────────────────────
CapToken  Rights    Scope                    Flags
──────────────────────────────────────────────────────────
@9f3c     rwxl---   /home/eric/              active
@3d8f     r-x----   /apps/exo-calendar/bin/  active
@2b9d     rw-----   /home/eric/.calendar/    active
@1f7b     r------   net:*.caldav.example.com active
@0011     r------   crypto_server            active

Total : 5 capabilities actives
Sandbox : non (app native)
ExoKairos budget : 45/100 ms/s utilisés
```

---

## 8. Règles d'Implémentation

### Ce qui est interdit dans les outils système ExoOS natifs

```
# INTERDIT
drwxr-xr-x  2 eric users 4096  documents/
-rwxr-xr-x  1 root root   2.1M  shell

# INTERDIT
Owner: eric (uid=1000)
Group: users (gid=1000)
Permissions: 755

# AUTORISÉ dans la couche musl-exo (compat uniquement)
# pour les apps POSIX qui en ont besoin
```

### Implémentation dans vfs_server

```rust
// vfs_server/src/display.rs

pub fn format_object_line(obj: &ExoObject, cap: CapToken) -> String {
    format!("{type}  {rights}  {flags:<10}  @{token:04x}  ep:{epoch:04}  {size:<10}  {hash:<8}  {name}",
        type   = obj.kind.char(),
        rights = format_rights(obj.get_rights_for(cap)),
        flags  = format_flags(obj.flags),
        token  = (cap.0 & 0xFFFF) as u16,
        epoch  = obj.epoch_id.0,
        size   = format_size(obj.size),
        hash   = obj.content_hash.map(|h| &hex(h)[..8]).unwrap_or("--------"),
        name   = obj.name,
    )
}

fn format_rights(r: RightsMask) -> String {
    let bits = [
        (Rights::Read,   'r'),
        (Rights::Write,  'w'),
        (Rights::Exec,   'x'),
        (Rights::List,   'l'),
        (Rights::Link,   'k'),
        (Rights::Seal,   's'),
        (Rights::Derive, 'd'),
    ];
    bits.iter().map(|(right, ch)|
        if r.contains(*right) { *ch } else { '-' }
    ).collect()
}
```

---

## 9. Audio — Affichage audio_server

```
$ exo ls -l /srv/audio_server

x  r-x----  [sealed][✓sig]  @0016  ep:0001  1.1 MiB  7a3b9f21  audio_server

$ exo stat /srv/audio_server

Sounds embarqués :
  BOOT_COMPLETE      88,200 bytes  (44100Hz stereo 0.5s)
  SECURITY_ALERT     88,200 bytes  (44100Hz stereo 1.0s)
  État : actif  HDA détecté  (PCI 00:1f.3)
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md*
