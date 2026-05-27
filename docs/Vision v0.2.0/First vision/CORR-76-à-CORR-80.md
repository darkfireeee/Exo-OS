# CORR-76 à CORR-80 — Bugs Kernel Critiques v0.1.0
## Corrections Bloquantes — À Appliquer Avant Toute Autre Chose

**Auteur :** claude-alpha  
**Date :** 2026-05-16  
**Source :** Audit claude-beta (bugs non adressés dans le corpus initial)

---

## CORR-76 — CRIT-01 : `map_physmap()` jamais appelée

**Fichier :** `kernel/src/memory/physmap.rs` + `kernel/src/boot/init.rs`

**Problème :** La fonction `map_physmap()` qui étend le mapping physique au-delà de 1 GiB n'est jamais appelée dans la séquence de boot. Sur toute machine avec > 1 GiB de RAM, `phys_to_virt()` sur une adresse > 1 GiB provoque un accès mémoire invalide → panique kernel.

**Localisation du fix :**

```rust
// kernel/src/boot/init.rs — dans memory_init(), APRÈS buddy_init()

pub fn memory_init(mem_map: &MemoryMap) -> Result<(), InitError> {
    // Étape 1 : Init du buddy allocator (inchangé)
    buddy_init(mem_map)?;
    
    // Étape 2 : NOUVEAU — Étendre la physmap à toute la RAM détectée
    let total_ram = mem_map.total_usable_bytes();
    if total_ram > PHYSMAP_INITIAL_COVERAGE {
        // PHYSMAP_INITIAL_COVERAGE = 1 GiB (coverage de démarrage)
        map_physmap(PHYSMAP_INITIAL_COVERAGE, total_ram)?;
        log::info!("physmap étendue : {} GiB total",
                   total_ram / (1024 * 1024 * 1024));
    }
    
    // Étape 3 : Suite inchangée (SLUB, vmalloc, etc.)
    slub_init()?;
    vmalloc_init()?;
    
    Ok(())
}
```

**Vérification :**
```rust
// const_assert! à ajouter dans physmap.rs
const _: () = assert!(
    PHYSMAP_INITIAL_COVERAGE == 1 * 1024 * 1024 * 1024,
    "PHYSMAP_INITIAL_COVERAGE doit être 1 GiB"
);
```

**Test :** Boot avec `-m 2G` sous QEMU → pas de panique.

---

## CORR-77 — CRIT-02 : `cgroup::init()` omis

**Fichier :** `kernel/src/scheduler/init.rs`

**Problème :** `cgroup::init()` n'est pas appelé dans la séquence d'initialisation du scheduler. Le root cgroup est invalide → le scheduler ne peut pas attacher les processus Ring1 au root cgroup → comportement indéfini au démarrage des serveurs.

**Fix :**

```rust
// kernel/src/scheduler/init.rs

pub fn scheduler_init() -> Result<(), InitError> {
    // NOUVEAU : initialiser les cgroups AVANT de créer le premier processus
    cgroup::init()?;
    let root_cgroup = cgroup::root();
    assert!(root_cgroup.is_valid(), "root cgroup invalide après init");
    
    // Suite existante (inchangée)
    runqueue_init()?;
    timer_init()?;
    idle_thread_init()?;
    
    // Attacher le thread idle au root cgroup
    cgroup::attach(idle_thread_pid(), root_cgroup)?;
    
    Ok(())
}
```

---

## CORR-78 — HIGH-01 : Injection PID via `len == 128`

**Fichier :** `kernel/src/ipc/core/dispatch.rs` (ou équivalent)

**Problème :** Un bloc de code utilise `len == 128` comme condition magique pour déterminer le type de message, permettant à un processus Ring3 de forger un PID en contrôlant la longueur de son message.

**Fix :**

```rust
// AVANT (vulnérable) :
fn dispatch_message(msg: &RawMessage) -> MessageType {
    if msg.len == 128 {  // ← MAGIC NUMBER = vecteur d'injection
        MessageType::PidMessage(extract_pid(msg))
    } else {
        MessageType::DataMessage
    }
}

// APRÈS (correct) :
fn dispatch_message(msg: &RawMessage) -> MessageType {
    // Utiliser le champ TYPE explicite du header, jamais la longueur
    match msg.header.msg_type {
        MSG_TYPE_PID => {
            // Vérifier que l'émetteur a la capability d'envoyer un PID message
            if capability::verify(msg.sender_cap, IpcRights::SEND_PID).is_err() {
                return MessageType::Invalid;
            }
            MessageType::PidMessage(extract_pid(msg))
        }
        MSG_TYPE_DATA => MessageType::DataMessage,
        _ => MessageType::Invalid,
    }
}
```

**Règle à ajouter dans `ipc_policy.rs` :**
```rust
// IPC-RULE-01 : Jamais utiliser la longueur d'un message comme discriminant de type.
// IPC-RULE-02 : Le champ msg_type du header est la seule source de vérité.
```

---

## CORR-79 — HIGH-02 : Service non-critique bloque `exosh`

**Fichier :** `kernel/src/process/lifecycle/init_server.rs`

**Problème :** `init_server` attend de manière synchrone que tous les services Ring1 soient prêts avant de lancer `exosh`. Si `network_server` timeout (pas de carte réseau, QEMU sans `-net`), `exosh` ne démarre jamais.

**Fix — Two-tier startup :**

```rust
// Dans init_server — séquence de démarrage corrigée

pub fn startup_sequence() -> Result<(), InitError> {
    // TIER 1 : Services CRITIQUES — exosh ne peut pas fonctionner sans eux
    // Timeout strict : 2 secondes chacun
    start_and_wait_critical(&[
        ServiceId::IpcBroker,
        ServiceId::MemoryServer,
        ServiceId::VfsServer,
        ServiceId::CryptoServer,
    ], Duration::from_secs(2))?;
    
    // TIER 2 : Services OPTIONNELS — démarrés en arrière-plan
    // exosh démarre SANS attendre ces services
    start_background(&[
        ServiceId::NetworkServer,  // ← ne bloque plus exosh
        ServiceId::DeviceServer,
        ServiceId::FbServer,
        ServiceId::ExoShield,
        ServiceId::MonitorServer,
    ]);
    
    // Lancer exosh immédiatement après les services Tier 1
    start_exosh()?;
    
    // Les services Tier 2 s'enregistrent quand ils sont prêts
    // exosh affiche un indicateur si un service Tier 2 n'est pas prêt
    Ok(())
}
```

**Dans exosh — message si service Tier 2 absent :**
```
ExoOS v0.2.0 — exosh 0.1.0
[WARN]  network_server : non disponible (démarrage en cours...)
[INFO]  Toutes les commandes locales disponibles. Réseau indisponible.
$ _
```

---

## CORR-80 — HIGH-03 : `USER_ELF_BASE_MIN` = 1 TiB rejette tous les ELF standards

**Fichier :** `kernel/src/process/lifecycle/exec.rs` ou `elf_loader_impl.rs`

**Problème :** `USER_ELF_BASE_MIN` est défini à 1 TiB (0x10000000000). Tous les ELF compilés avec les paramètres standard ont une adresse de base de 0x400000 (4 MiB). Le loader rejette donc **tous** les binaires ELF standard avec une erreur "base address too low".

**Fix :**

```rust
// AVANT (incorrect) :
const USER_ELF_BASE_MIN: u64 = 0x10000000000;  // 1 TiB — trop haut

// APRÈS (correct) :
/// Adresse minimale de chargement ELF pour les processus Ring3.
/// 4 MiB : évite la zone basse (NULL, vecteurs, stack boot)
/// Compatible avec les ELF standard compilés à 0x400000.
const USER_ELF_BASE_MIN: u64 = 0x400000;  // 4 MiB

/// Adresse maximale pour l'espace utilisateur (Ring3).
/// Laisse la moitié haute du VA space au kernel.
const USER_ELF_BASE_MAX: u64 = 0x7FFF_FFFF_F000;  // ~128 TiB
```

**const_assert! à ajouter :**
```rust
const _: () = assert!(
    USER_ELF_BASE_MIN < USER_ELF_BASE_MAX,
    "USER_ELF_BASE_MIN doit être < USER_ELF_BASE_MAX"
);
const _: () = assert!(
    USER_ELF_BASE_MIN <= 0x400000,
    "USER_ELF_BASE_MIN trop haut — les ELF standard ont base 0x400000"
);
```

**Test :**
```bash
# Compiler un binaire ELF minimal
echo 'fn main() {}' | rustc --target x86_64-unknown-none - -o /tmp/test_elf
# Charger avec le loader ExoOS → PASS
```

---

*claude-alpha — ExoOS v0.2.0 — CORR-76-à-CORR-80.md*
