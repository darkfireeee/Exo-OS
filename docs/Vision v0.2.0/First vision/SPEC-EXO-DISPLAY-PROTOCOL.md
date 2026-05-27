# SPEC-EXO-DISPLAY-PROTOCOL — Protocole d'Affichage ExoOS
## Format Natif des Données, Permissions et Métadonnées

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0

---

## 1. Problème

Linux affiche les permissions sous forme POSIX : `drwxr-xr-x  2 eric users 4096`.

ExoOS n'a ni `rwx` traditionnel, ni `uid`, ni `gid`, ni inode numéro. Utiliser ce format serait un mensonge architectural — il cacherait le modèle réel derrière une façade qui n'existe pas.

ExoOS doit définir **son propre protocole d'affichage** cohérent avec ses primitives.

---

## 2. Modèle Mental ExoOS

Tout objet dans ExoOS a :

| Attribut | Type | Description |
|---|---|---|
| **Type** | `ObjectKind` | blob / dir / relation / secret / code / config |
| **Capabilities** | `RightsMask` | read / write / exec / list / link / seal / derive |
| **Token** | `CapToken` | identifiant de possession (u64 opaque) |
| **Epoch** | `EpochId` | version dans ExoFS (monotone) |
| **Flags** | `ObjectFlags` | sealed / signed / encrypted / pinned / immutable |
| **Taille** | `u64` | en octets (pour blobs) ou nombre d'entrées (pour dirs) |
| **Hash** | `ContentHash` | BLAKE3 du contenu (content-addressed) |
| **Nom** | `PathComponent` | composant de chemin UTF-8 |

---

## 3. Format Long (`exo ls -l`)

### 3.1 Syntaxe

```
<type> <rights> <flags>  <token>      <epoch>       <size>    <hash-short>  <nom>
```

### 3.2 Caractères de Type

| Char | Signification |
|------|--------------|
| `d` | Répertoire (directory — PathIndex dans ExoFS) |
| `b` | Blob de données (fichier ordinaire) |
| `r` | Relation typée (lien sémantique entre objets) |
| `s` | Secret (contenu chiffré, jamais lisible directement) |
| `x` | Code exécutable (blob + flag exec) |
| `c` | Configuration (blob + format structuré connu) |
| `?` | Type inconnu ou non résolu |

### 3.3 Masque de Droits (7 bits)

Format : `rwxlksd` où chaque position est le droit ou `-` si absent.

| Position | Lettre | Signification |
|----------|--------|--------------|
| 1 | `r` | **read** — lire le contenu |
| 2 | `w` | **write** — modifier le contenu |
| 3 | `x` | **exec** — exécuter (code) ou traverser (dir) |
| 4 | `l` | **list** — lister les entrées (dirs) |
| 5 | `k` | **link** — créer des relations vers cet objet |
| 6 | `s` | **seal** — sceller (rendre immutable) |
| 7 | `d` | **derive** — dériver des sous-capabilities |

### 3.4 Flags d'État

| Code | Signification |
|------|--------------|
| `[sealed]` | Objet scellé — immuable depuis l'epoch indiquée |
| `[✓sig]` | Signature cryptographique vérifiée |
| `[enc]` | Contenu chiffré (xchacha20 par objet) |
| `[pin]` | Épinglé — non éligible au GC |
| `[snap]` | Snapshot — lecture seule depuis une epoch |
| `[ghost]` | Supprimé logiquement, en attente de GC |
| `·` | Aucun flag particulier |

### 3.5 Token

Format court : `@` suivi des 4 derniers octets du CapToken en hex.

```
@1a2b  →  CapToken dont les 4 LSB = 0x1a2b****
```

Le token complet est visible avec `exo stat <obj>`.

### 3.6 Exemples Complets

```
$ exo ls -l /home/eric/

d  rwxl---  ·         @9f3c  ep:0042  4 entries     --------  documents/
b  rw-----  [enc]     @7a1e  ep:0041  124.3 KiB     a3f2b1c8  rapport.exo
x  r-x----  [✓sig]    @3d8f  ep:0038  2.1 MiB       9e4a72f1  shell
s  r------  [enc]     @c02a  ep:0040  256 B         --------  api_key
r  r----k-  ·         @5511  ep:0039  → documents/  --------  docs_link
c  rw-----  ·         @2b9d  ep:0043  1.4 KiB       f7c3a091  config.exo
```

### 3.7 Format Court (`exo ls`)

```
$ exo ls /home/eric/

d  documents/
b  rapport.exo      [enc]
x  shell            [✓sig]
s  api_key          [enc]
r  docs_link        → documents/
c  config.exo
```

---

## 4. Format de Stat (`exo stat`)

```
$ exo stat /home/eric/rapport.exo

Object: rapport.exo
  Type        : blob
  ObjectID    : 0xdeadbeef0001cafe
  CapToken    : 0x9a3f7c1e2b5d8a0f  [HOLDER]
  Rights      : read write  (rw-----)
  Flags       : encrypted
  Encryption  : xchacha20-poly1305 / key@crypto_server:0x4421
  Size        : 127,318 bytes  (124.3 KiB)
  ContentHash : BLAKE3:a3f2b1c8e9d74f5601a2b3c4d5e6f789...
  Epoch       : 41  (created: ep:12, modified: ep:41)
  EpochDate   : 2026-05-14T22:30:01Z
  Snapshots   : 3  (ep:15, ep:28, ep:41)
  Relations   : 1 incoming (from: docs_link@0x5511)
  RefCount    : 2  (1 direct, 1 via relation)
  ExoPhoenix  : resurrection-safe  (stateless blob)
```

---

## 5. Format de Processus (`exo ps`)

Pas de `uid/gid`. L'identité d'un processus = son ensemble de capabilities.

```
$ exo ps

PID   RING  STATE    CAPS                          CPU%  MEM      NAME
1     R1    running  [ipc_broker:full]             0.1%  128KiB   ipc_broker
2     R1    running  [memory:full]                 0.3%  256KiB   memory_server
3     R1    running  [vfs:full,block:rw]           1.2%  4.8MiB   vfs_server
4     R1    running  [crypto:full,trng:r]          0.8%  2.1MiB   crypto_server
5     R1    running  [net:full,dma:rw]             2.3%  8.4MiB   network_server
6     R1    running  [dev:full,pci:rw]             0.4%  1.6MiB   device_server
42    R3    running  [fs:r--l,ipc:send]            0.0%  1.2MiB   calendar
43    R3    blocked  [fs:rw-l,net:r,ipc:send]      0.0%  3.8MiB   curl
```

Colonnes :
- `PID` : identifiant de processus ExoOS (opaque)
- `RING` : niveau d'exécution (R0/R1/R3)
- `STATE` : running / blocked / sleeping / zombie
- `CAPS` : résumé des capabilities actives (format `[domaine:droits]`)
- `CPU%` : utilisation CPU
- `MEM` : mémoire physique allouée
- `NAME` : nom du binaire

---

## 6. Format d'IPC (`exo ipc stat`)

```
$ exo ipc stat

CHANNEL  TYPE    PRODUCER    CONSUMER      MSGS/S   LATENCY  BACKLOG
ipc:001  SPSC    vfs_server  calendar      12,400   8µs      0/16
ipc:002  MPMC    net_server  [multi]       890,000  2µs      3/16
ipc:003  RPC     calendar    vfs_server    340      45µs     0/1
```

---

## 7. Format des Capabilities (`exo cap list`)

```
$ exo cap list --pid 42

Capabilities du processus 42 (calendar):

TOKEN       DOMAIN      RIGHTS      SCOPE              EXPIRY
0x9a3f7c1e  fs          r--l----    /home/eric/cal/    never
0x3b2d8f01  ipc         --send--    network_server     session
0x7c1e4a2b  time        r-------    system_clock       never
0x4f8a9c3d  display     ---w----    fb_server:region   session

Capabilities refusées (tentatives récentes):
  [DENIED]  0x0000:  fs:write  /etc/  →  ExoLedger#4421
  [DENIED]  0x0000:  net:connect  0.0.0.0:22  →  ExoKairos#budget_exceeded
```

---

## 8. Format des Erreurs

ExoOS n'affiche pas "Permission denied". Il indique précisément ce qui manque :

```
Erreur:  EXO-0403  CAPABILITY_INSUFFICIENT
  Processus:  curl (PID 43)
  Action:     write → /etc/hosts
  Manque:     cap[fs:write] sur scope /etc/
  Possède:    cap[fs:rw-l] sur scope /home/eric/ uniquement
  Audit:      ExoLedger#4422  (2026-05-14T22:31:05Z)
  Conseil:    exo cap request fs:write /etc/hosts [--justify "raison"]
```

Format général :
```
Erreur:  EXO-<code>  <NOM_ERREUR>
  [contexte spécifique]
  Audit:  ExoLedger#<id>
  Conseil: <action suggérée>
```

Codes d'erreur ExoOS (extrait) :

| Code | Nom | Signification |
|------|-----|--------------|
| `EXO-0403` | `CAPABILITY_INSUFFICIENT` | Capability manquante pour l'action |
| `EXO-0404` | `OBJECT_NOT_FOUND` | Objet inexistant dans ExoFS |
| `EXO-0409` | `EPOCH_CONFLICT` | Conflit de version (concurrent write) |
| `EXO-0410` | `CAPABILITY_REVOKED` | Capability révoquée depuis l'obtention |
| `EXO-0429` | `BUDGET_EXCEEDED` | ExoKairos : quota temporel dépassé |
| `EXO-0500` | `SERVER_FAULT` | Erreur interne serveur Ring1 |
| `EXO-0503` | `SERVER_UNAVAILABLE` | Serveur Ring1 non démarré |
| `EXO-0507` | `PHOENIX_SWITCHING` | Bascule ExoPhoenix en cours |

---

## 9. Format de Log (`exo log`)

```
$ exo log --last 10

2026-05-14T22:31:05Z  [INFO]   calendar(42)    fs:open  /home/eric/cal/2026.exo  cap@9a3f
2026-05-14T22:31:05Z  [INFO]   calendar(42)    fs:read  124 bytes  ep:41
2026-05-14T22:31:05Z  [AUDIT]  curl(43)        cap:denied  fs:write /etc/hosts  →  ledger#4422
2026-05-14T22:31:06Z  [INFO]   network(5)      tcp:connect  93.184.216.34:443  via cap@3b2d
2026-05-14T22:31:06Z  [INFO]   crypto(4)       tls:handshake  ok  TLS1.3  ECDHE-ChaCha20
2026-05-14T22:31:06Z  [WARN]   network(5)      dns:timeout  retry 1/3
2026-05-14T22:31:07Z  [INFO]   network(5)      dns:resolved  example.com → 93.184.216.34
2026-05-14T22:31:07Z  [INFO]   vfs(3)          exofs:epoch  commit ep:43  ✓  hash:a3f2b1c8
2026-05-14T22:31:08Z  [INFO]   exophoenix      heartbeat  kernel-A  ssr:ok  latency:0.2ms
2026-05-14T22:31:09Z  [INFO]   exoledger       audit:flush  4423 entries  signed  ep:43
```

---

## 10. Règles d'Implémentation

1. **Jamais** afficher `uid`, `gid`, `rwx` traditionnel dans aucun outil système ExoOS natif
2. **Toujours** afficher le type ExoFS (b/d/r/s/x/c) comme premier caractère
3. **Toujours** afficher le token abrégé `@XXXX` pour traçabilité
4. **Jamais** simuler des permissions POSIX dans la couche musl-exo visible — la compat POSIX est interne
5. Les outils POSIX portés (ex: `ls` de busybox) **peuvent** afficher le format POSIX dans leur sandbox, mais `exo ls` doit toujours afficher le format ExoOS
6. Les messages d'erreur doivent **toujours** inclure un identifiant ExoLedger pour auditabilité

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-DISPLAY-PROTOCOL.md*
