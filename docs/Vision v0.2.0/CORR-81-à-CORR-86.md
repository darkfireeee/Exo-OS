# CORR-81 à CORR-86 — Erreurs de Specs + VirtIO Disk
## Corrections des Erreurs Techniques dans le Corpus claude-alpha

**Auteur :** claude-alpha  
**Date :** 2026-05-16

---

## CORR-81 — ERR-01 : SSR Redesign (overflow 4 KiB)

**Document original :** `SPEC-EXOPHOENIX-V0.2.md` §3.2

**Calcul réel de la struct originale :**
```
Header + divers        :    61 octets
cap_table refs         :    44 octets
[ProcessRecord; 64]    : 7 424 octets  ← dépasse seul la page
[EndpointRecord; 128]  : 2 564 octets
Timing + padding       :    16 octets
─────────────────────────────────────
TOTAL                  : ~10 109 octets  ← 2.5× la page de 4 KiB
```

**SSR Redesigné — Contraintes Réelles :**

Le SSR doit tenir dans une zone physique réservée connue et non fragmentée. La solution est de **ne pas se contraindre à 4 KiB** mais de calculer et documenter la taille réelle.

```rust
// kernel/src/exophoenix/ssr.rs — VERSION CORRIGÉE

/// Nombre maximum de processus restaurables lors d'une bascule.
/// Priorité : Ring1 servers (≤12) + Ring3 critiques.
/// Politique : si > SSR_MAX_PROCESSES → abandon documenté par ordre PID croissant.
pub const SSR_MAX_PROCESSES: usize = 24;  // ← était 64 (impossible en 4KiB)

/// Nombre maximum d'endpoints IPC à préserver.
pub const SSR_MAX_ENDPOINTS: usize = 48;  // ← était 128

/// Taille d'un ProcessRecord (calculée explicitement).
/// pid:u32 + ring:u8 + restore_mode:u8 + _pad:u16 + binary_hash:[u8;32]
/// + cap_bitmap:u64 + restart_args:[u8;48] = 4+1+1+2+32+8+48 = 96 octets
pub const PROCESS_RECORD_SIZE: usize = 96;

/// Taille d'un EndpointRecord.
/// endpoint_id:u32 + owner_pid:u32 + cap:u64 + _pad:u64 = 24 octets
pub const ENDPOINT_RECORD_SIZE: usize = 24;

/// Taille totale du SSR — calculée statiquement.
pub const SSR_SIZE: usize =
      64   // Header (magic + version + hash + active_kernel + epoch + boot_count + reason)
    + 44   // cap_table (ptr + len + hash)
    +  4   // process_count
    + SSR_MAX_PROCESSES * PROCESS_RECORD_SIZE    // 24 × 96 = 2 304 octets
    +  4   // endpoint_count
    + SSR_MAX_ENDPOINTS * ENDPOINT_RECORD_SIZE   // 48 × 24 = 1 152 octets
    + 16   // timing (switch_start_ns + switch_end_ns)
    ;
// SSR_SIZE ≈ 3 588 octets — tient dans 4 KiB, vérifié statiquement

const _: () = assert!(SSR_SIZE <= 4096, "SSR dépasse 4 KiB — réduire les limites");
const _: () = assert!(SSR_MAX_PROCESSES >= 12, "SSR doit pouvoir restaurer au moins 12 Ring1 servers");

/// Zone physique réservée pour le SSR (marquée dans E820).
/// Layout : [0x1000000 .. 0x1001000] = 4 KiB à 16 MiB physique.
pub const SSR_PHYS_BASE: u64    = 0x1000000;
pub const SSR_PHYS_END:  u64    = SSR_PHYS_BASE + 4096;

#[repr(C, align(4096))]
pub struct SystemStateRecord {
    // Header : 64 octets
    pub magic:         u32,
    pub version:       u32,
    pub ssr_hash:      [u8; 32],
    pub active_kernel: KernelId,
    pub epoch_id:      EpochId,
    pub boot_count:    u64,
    pub switch_reason: SwitchReason,

    // Capabilities : 44 octets
    pub cap_table_ptr:  PhysAddr,
    pub cap_table_len:  u32,
    pub cap_table_hash: [u8; 32],

    // Processus : 4 + 24×96 = 2 308 octets
    pub process_count: u32,
    pub processes:     [ProcessRecord; SSR_MAX_PROCESSES],

    // Endpoints : 4 + 48×24 = 1 156 octets
    pub endpoint_count: u32,
    pub endpoints:      [EndpointRecord; SSR_MAX_ENDPOINTS],

    // Timing : 16 octets
    pub switch_start_ns: u64,
    pub switch_end_ns:   u64,
}

// Vérification statique de la taille totale
const _ASSERT_SSR_SIZE: () = assert!(
    core::mem::size_of::<SystemStateRecord>() <= 4096,
    "SystemStateRecord dépasse 4096 octets"
);
```

**Politique de priorisation (SSR_MAX_PROCESSES = 24) :**
```
1. Ring1 servers (toujours : ≤12 servers)  → slots 0..11
2. Ring3 avec cap PHOENIX_PERSIST          → slots 12..19
3. Ring3 standard par PID croissant        → slots 20..23
4. Au-delà de 24 → abandon + ExoLedger entry
```

---

## CORR-82 — ERR-02 : Séquence de Boot Corrigée

**Document original :** `SPEC-EXO-SECURITY-ACTIVATION.md` §5

**Séquence Corrigée (remplace la Section 5 complète) :**

```
Phase 0:  memory_init()
          ├── buddy_init(mem_map)
          ├── map_physmap() [CORR-76]           ← physmap complète
          ├── slub_init()
          └── vmalloc_init()

Phase 1:  arch_init()
          ├── gdt_init()
          ├── idt_init()
          ├── apic_init()                        ← LAPIC disponible après cette étape
          └── tsc_calibrate()

Phase 2:  ExoCage activate_hardware()
          ├── Activer SMEP/SMAP (CR4)            ← pas de heap requis
          ├── Activer NX/XD (EFER.NXE)
          ├── Activer IBRS/SSBD (MSR_SPEC_CTRL)
          └── Activer CET (MSR_IA32_S_CET)

Phase 3:  ExoNMI arm_watchdog()                  ← LAPIC disponible (Phase 1)
          └── NMI toutes les 200ms via LAPIC timer

Phase 4:  scheduler_init()
          ├── cgroup::init() [CORR-77]           ← root cgroup valide
          ├── runqueue_init()
          └── ExoKairos init (budgets temporels)
              └── avec reset fenêtre [ERR-07 fix]

Phase 5:  security_init() complet
          ├── integrity_check()
          ├── capability::init()
          ├── zero_trust_init()
          │   └── IPC fast path : bitmask précompilé [ERR-09 fix]
          ├── crypto_init() (RDRAND + ChaCha20)
          ├── isolation_init() (pledge, sandbox)
          ├── mitigations_init() (KASLR, CFG, SafeStack)
          ├── audit_init()
          ├── access_control_init()
          └── exoledger_init()
              └── is_immutable() vérifié [ERR-04 fix]

Phase 6:  ExoSeal verify_boot_chain()
          ├── blake3_hash_kernel_image()          ← heap disponible (Phase 0)
          ├── tpm_read_expected_hash()            ← PCI bus disponible (Phase 1)
          └── SEAL_STATE = Verified

Phase 7:  ExoShield configure_iommu()
          └── AVANT tout démarrage driver Ring1

Phase 8:  ExoArgos init (PMC hook)
Phase 9:  ipc_init() avec ZeroTrust labels
Phase 10: VirtIO BAR correct [CORR-86] + ExoFS mount
Phase 11: Ring1 servers (Tier 1 critiques d'abord) [CORR-79]
Phase 12: exosh
Phase 13: Ring1 servers Tier 2 en arrière-plan
Phase 14: SECURITY_READY.store(Release)
```

**ExoKairos — Reset de fenêtre (ERR-07) inclus ici :**

```rust
// kernel/src/security/exokairos.rs — CORRECTION

pub struct KairosBudget {
    pub used_ns:          u64,
    pub limit_ns:         u64,   // 100ms en ns = 100_000_000
    pub limit_200pct_ns:  u64,   // 200ms en ns = 200_000_000
    pub window_start_ns:  u64,   // ← NOUVEAU : début de la fenêtre courante
}

pub const KAIROS_WINDOW_NS: u64 = 1_000_000_000;  // 1 seconde

fn update_kairos_budget(tcb: &mut Tcb, elapsed_ns: u64, now_ns: u64) {
    let budget = &mut tcb.kairos_budget;

    // Reset fenêtre si expirée
    if now_ns.saturating_sub(budget.window_start_ns) >= KAIROS_WINDOW_NS {
        budget.used_ns         = 0;
        budget.window_start_ns = now_ns;
    }

    budget.used_ns += elapsed_ns;

    if budget.used_ns > budget.limit_200pct_ns {
        kairos_kill(tcb);    // 200ms dépassé dans la fenêtre → kill
    } else if budget.used_ns > budget.limit_ns {
        kairos_throttle(tcb); // 100ms dépassé → throttle
    }
}
```

**Zero Trust fast path (ERR-09) inclus ici :**

```rust
// kernel/src/security/zero_trust/check.rs

/// Vérification IPC — deux niveaux selon la source
pub fn check_ipc(src: Pid, dst: Pid, action: IpcAction) -> ZeroTrustResult {
    let src_class = ipc_policy::class_of(src);
    let dst_class = ipc_policy::class_of(dst);

    // FAST PATH : Ring1↔Ring1 avec bitmask précompilé (O(1), 1-2 cycles)
    if src_class.is_ring1() && dst_class.is_ring1() {
        let allowed = RING1_TRUST_MATRIX[src_class as usize][dst_class as usize];
        return if allowed { ZeroTrustResult::Allow } else { ZeroTrustResult::Deny };
    }

    // SLOW PATH : Ring3 → vérification complète avec lookup table
    let src_label = get_label(src)?;
    let dst_label = get_label(dst)?;
    verify_bell_lapadula(&src_label, &dst_label, action)?;
    verify_biba(&src_label, &dst_label, action)?;

    ZeroTrustResult::Allow
}

/// Matrice de confiance Ring1↔Ring1 précompilée au boot — immuable après cfg_lock()
static RING1_TRUST_MATRIX: [[bool; ServiceClass::COUNT]; ServiceClass::COUNT] = {
    /* ... initialisée dans security_init() ... */
    [[false; ServiceClass::COUNT]; ServiceClass::COUNT]
};
```

---

## CORR-83 — ERR-03 : Graphics v0.2.0 sans wgpu

**Document original :** `SPEC-EXO-GRAPHICS.md`

**wgpu est retiré de v0.2.0.** Il est impossible de compiler wgpu en no_std.

**Stack graphique v0.2.0 révisée :**

```
v0.2.0 (ce document) :
  fb_server Ring1 — framebuffer GOP UEFI
  fontdue   Ring3 — rendu de texte no_std (police bitmap ou TrueType)
  exosh     Ring3 — terminal texte sur framebuffer
  winit     Ring3 — REPORTÉ (pas de Wayland)

v0.3.0 (futur) :
  Wayland compositor
  wgpu avec musl-exo std complet (Ring3 avec std disponible)
  iced (dépend de wgpu)
```

**`fontdue` — Rendu de texte no_std :**
```toml
# exosh/Cargo.toml
[dependencies]
fontdue = { version = "0.9", default-features = false }  # no_std compatible
```

```rust
// exosh/src/renderer.rs — rendu texte sur GOP framebuffer

use fontdue::{Font, FontSettings};

pub struct TextRenderer {
    font:   Font,
    fb:     &'static mut [u32],  // framebuffer GOP linéaire
    width:  usize,
    height: usize,
}

impl TextRenderer {
    pub fn render_char(&mut self, ch: char, x: usize, y: usize, color: u32) {
        let (metrics, bitmap) = self.font.rasterize(ch, 14.0);
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let alpha = bitmap[row * metrics.width + col] as u32;
                if alpha > 128 {
                    let px = (y + row) * self.width + (x + col);
                    if px < self.fb.len() {
                        self.fb[px] = color;
                    }
                }
            }
        }
    }

    pub fn render_string(&mut self, s: &str, x: usize, y: usize, color: u32) {
        let mut cx = x;
        for ch in s.chars() {
            self.render_char(ch, cx, y, color);
            cx += 8;  // largeur fixe pour la police monospace
        }
    }

    pub fn blit_to_fb_server(&self) {
        // Notifier fb_server de recomposer la région
        let _ = ipc_send(FbEndpoint::ID, FbRequest::Blit {
            surface_id: self.surface_id,
            dirty_rect: None,
        });
    }
}
```

**Checklist graphics corrigée pour v0.2.0 :**
- [ ] `fb_server` Ring1 fonctionnel (GOP UEFI)
- [ ] `fontdue` compile en no_std dans exosh
- [ ] Rendu de texte ASCII complet sur framebuffer
- [ ] Prompt `$ ` interactif et stable
- [ ] `exo ls` → format capability affiché correctement
- [ ] Bascule ExoPhoenix → exosh redémarre, framebuffer re-rendu
- [-] wgpu → v0.3.0
- [-] iced → v0.3.0
- [-] winit → v0.3.0

---

## CORR-84 — ERR-04 : `is_immutable()` dans le chemin d'écriture

**Fichier :** `kernel/src/fs/exofs/syscall/blob_write.rs`

```rust
pub fn vfs_write_at(
    blob_id:    BlobId,
    offset:     u64,
    data:       &[u8],
    pid:        u32,
    write_cap:  &CapToken,
) -> ExofsResult<usize> {

    // Étape 1 : Vérifier l'immutabilité AVANT toute opération
    let meta = blob_meta_cache_get(blob_id)?;
    if meta.is_immutable() {
        // Auditer la tentative
        exoledger_append(LedgerEntry {
            pid,
            event: LedgerEvent::WriteAttemptOnImmutable { blob_id },
            result: LedgerResult::Deny,
            ..default_entry()
        });
        return Err(ExofsError::AccessDenied(AccessDeniedReason::Immutable));
    }

    // Étape 2 : Vérifier les capabilities (déjà existant)
    capability::verify(write_cap, FsRights::WRITE, ObjectId::from(blob_id))?;

    // Étape 3 : Vérifier l'epoch (déjà existant)
    let current_epoch = epoch_current();
    if meta.last_epoch > current_epoch {
        return Err(ExofsError::EpochConflict);
    }

    // Étape 4 : Écriture (inchangée)
    write_blob_data(blob_id, offset, data)?;
    Ok(data.len())
}
```

**Test requis :**
```rust
#[test]
fn test_immutable_write_rejected() {
    let blob = create_test_blob();
    set_immutable(blob.id);
    
    let result = vfs_write_at(blob.id, 0, b"tamper", 42, &test_cap());
    assert!(matches!(result, Err(ExofsError::AccessDenied(AccessDeniedReason::Immutable))));
    
    // Vérifier l'entrée ExoLedger
    let last_entry = exoledger_last();
    assert!(matches!(last_entry.event, LedgerEvent::WriteAttemptOnImmutable { .. }));
    assert_eq!(last_entry.result, LedgerResult::Deny);
}
```

---

## CORR-85 — ERR-05 : IPC Réseau > MAX_MSG_SIZE (240 octets)

**Document original :** `SPEC-EXO-CRATES.md` §2.2  
**MAX_MSG_SIZE = 240 octets** dans le kernel. MSS TCP = 1460 octets.

**Protocole corrigé — Deux niveaux :**

```rust
// exo-net/src/lib.rs — API Ring3 CORRIGÉE

/// Seuil entre inline et SHM transfer
const IPC_INLINE_MAX: usize = 200;  // < MAX_MSG_SIZE avec marge header

impl TcpStream {
    pub fn write(&mut self, data: &[u8]) -> Result<usize, NetError> {
        if data.len() <= IPC_INLINE_MAX {
            // CHEMIN COURT : données dans le payload IPC
            self.write_inline(data)
        } else {
            // CHEMIN LONG : données via SHM
            self.write_shm(data)
        }
    }

    fn write_inline(&mut self, data: &[u8]) -> Result<usize, NetError> {
        debug_assert!(data.len() <= IPC_INLINE_MAX);
        let mut payload = [0u8; IPC_INLINE_MAX];
        payload[..data.len()].copy_from_slice(data);
        ipc_send_recv(NetEndpoint::ID, NetRequest::WriteInline {
            handle: self.socket_handle,
            cap:    self.cap,
            len:    data.len() as u16,
            data:   payload,
        }).map(|r| r.bytes_written)
    }

    fn write_shm(&mut self, data: &[u8]) -> Result<usize, NetError> {
        // 1. Allouer un SHM segment
        let (shm_virt, shm_cap) = sys_shm_create(data.len())?;

        // 2. Copier les données dans le SHM (côté Ring3)
        let shm_slice = unsafe {
            core::slice::from_raw_parts_mut(shm_virt.as_ptr(), data.len())
        };
        shm_slice.copy_from_slice(data);

        // 3. Envoyer un IPC court avec la référence SHM
        let result = ipc_send_recv(NetEndpoint::ID, NetRequest::WriteSHM {
            handle:  self.socket_handle,
            cap:     self.cap,
            shm_cap: shm_cap,
            offset:  0,
            len:     data.len(),
        })?;

        // 4. Libérer le SHM (network_server l'a lu)
        sys_shm_release(shm_cap)?;

        Ok(result.bytes_written)
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        // READ : même logique — inline si petit, SHM si grand
        if buf.len() <= IPC_INLINE_MAX {
            self.read_inline(buf)
        } else {
            self.read_shm(buf)
        }
    }
}
```

**Côté network_server (Ring1) :**
```rust
NetRequest::WriteSHM { handle, cap, shm_cap, offset, len } => {
    // Vérifier la capability SHM
    capability::verify(&shm_cap, ShmRights::READ, ...)?;
    // Mapper le SHM dans l'espace d'adressage du serveur
    let data = shm_map_read(shm_cap, offset, len)?;
    // Envoyer via smoltcp
    smoltcp_send(handle, data)?;
    // Relâcher le mapping
    shm_unmap(shm_cap);
    ipc_reply(NetResponse { bytes_written: len });
}
```

---

## CORR-86 — C-GAMMA-01 : VirtIO BAR Incorrect → ExoFS RAM-only

**Source :** claude-gamma — découverte critique  
**Fichier :** `kernel/src/drivers/virtio/virtio_adapter.rs`

**Problème :** L'adresse VirtIO hardcodée `0x1000_0000` = borne haute de la RAM avec `-m 256M`. Le BAR PCI réel de `virtio-blk-pci` est à ~`0xC000_0000`. ExoFS ne persiste rien sur disque.

**Fix — Lire le BAR depuis le PCI config space :**

```rust
// kernel/src/drivers/virtio/virtio_adapter.rs — CORRECTION

// AVANT (incorrect) :
const VIRTIO_BLK_MMIO_BASE: u64 = 0x1000_0000;  // ← FAUX : borne RAM !

// APRÈS : lire le BAR dynamiquement depuis PCI config space
pub fn virtio_blk_init(pci_bus: u8, pci_dev: u8, pci_fn: u8) -> Result<VirtioBlkDriver, DriverError> {
    let pci = PciDevice::new(pci_bus, pci_dev, pci_fn);

    // Lire BAR0 depuis le PCI config space (offset 0x10)
    let bar0_raw = pci.read_config_u32(PCI_BAR0_OFFSET);

    let mmio_base = if bar0_raw & 0x1 == 0 {
        // Memory BAR
        let base = (bar0_raw & 0xFFFFFFF0) as u64;

        // Pour BAR 64-bit : lire aussi BAR1 (offset 0x14)
        if (bar0_raw >> 1) & 0x3 == 0x2 {
            let bar1_raw = pci.read_config_u32(PCI_BAR1_OFFSET);
            base | ((bar1_raw as u64) << 32)
        } else {
            base
        }
    } else {
        return Err(DriverError::IoBarNotSupported);
    };

    log::info!(
        "virtio-blk PCI {:02x}:{:02x}.{} — BAR0 MMIO base: {:#x}",
        pci_bus, pci_dev, pci_fn, mmio_base
    );

    // Mapper la région MMIO dans le VA space kernel
    let mmio_virt = mm::ioremap(PhysAddr::new(mmio_base), VIRTIO_BLK_MMIO_SIZE)?;

    Ok(VirtioBlkDriver::new(mmio_virt))
}
```

**Ordre d'implémentation imposé par cette correction :**

```
PHASE 0.0  Corriger VirtIO BAR (ce CORR-86)
PHASE 0.1  Boot QEMU avec -m 256M + virtio-blk-pci
PHASE 0.2  Vérifier BAR lu correctement dans le log :
           "virtio-blk PCI 00:03.0 — BAR0 MMIO base: 0xC0000000"
PHASE 0.3  Valider ExoFS sur disque :
           exosh:/$ echo "test" > /test.txt
           exosh:/$ reboot
           exosh:/$ cat /test.txt   → "test" (persisté !)
PHASE 0.4  SEULEMENT APRÈS : commencer musl-exo, exo-pkg, etc.
```

**Test de non-régression :**
```bash
# QEMU avec disque attaché
qemu-system-x86_64 \
  -m 256M \
  -device virtio-blk-pci,drive=exofs0 \
  -drive id=exofs0,file=exofs-root.img,format=raw,if=none \
  -serial stdio

# Dans exosh, vérifier la persistance :
$ echo "persistence_test" > /data/test.txt
$ sync
# Ctrl+Alt+Del → reboot
$ cat /data/test.txt
persistence_test   ← doit afficher ça
```

---

*claude-alpha — ExoOS v0.2.0 — CORR-81-à-CORR-86.md*
