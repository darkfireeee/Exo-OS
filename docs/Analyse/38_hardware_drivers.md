# Rapport d'Analyse : Pilotes Matériels Userspace (drivers/)

## 1. Vue d'Ensemble

Le sous-système `drivers/` contient l'ensemble des pilotes de périphériques exécutés en espace utilisateur (Ring 1) conformément à l'architecture micro-noyau GI-03 d'Exo-OS. Ces pilotes interagissent avec le matériel via des régions MMIO et des interruptions isolées par l'IOMMU, sans exécuter de code arbitraire en Ring 0.

### Catégories de Pilotes :
1. **Entrée / Sortie :**
   - `drivers/input/ps2` : Contrôleur clavier et souris i8042 (ports I/O 0x60 / 0x64).
   - `drivers/input/usb_hid` : Pilote USB Human Interface Device pour claviers et souris USB.
   - `drivers/tty` : Pilote de port série UART 16550/8250 (COM1 0x3F8) pour console et débogage.
2. **Affichage & Graphisme :**
   - `drivers/display/vga` : Contrôleur VGA standard (registres CRT, séquenceur, DAC palette, mémoire tampon 0xB8000).
   - `drivers/gpu/virtio_gpu` : Pilote GPU virtualisé VirtIO avec accélération de rendu 2D et gestion de scans virtuels.
3. **Réseau :**
   - `drivers/network/e1000` : Contrôleur Ethernet Gigabit Intel 82540EM PCI (descripteurs DMA RX/TX en anneau).
   - `drivers/network/virtio_net` : Adaptateur réseau VirtIO (virtqueues TX/RX, checksum offloading).
   - `drivers/network/loopback` : Interface virtuelle de boucle locale (127.0.0.1).
   - `drivers/network/common` : Abstraction commune de paquets et buffers réseau.
4. **Stockage & Persistance :**
   - `drivers/storage/nvme` : Pilote NVM Express PCIe haute performance (files de soumission et complétion SQ/CQ, portes de sonnette doorbell).
   - `drivers/storage/ahci` : Pilote SATA AHCI (ports HBA, tables PRDT pour le DMA bus-mastering).
   - `drivers/storage/common_hal` : Trait d'abstraction `BlockDeviceOps` unifiant l'accès bloc.
5. **Sécurité :**
   - `drivers/security/verity` : Moteur de vérification cryptographique Ed25519 + SHA-512 partagé avec `exo-boot` et `kernel_signer`.
6. **Audio :**
   - `drivers/audio` : Contrôleur audio AC97 / Intel High Definition Audio (buffers cycliques DMA).

---

## 2. Analyse Ligne par Ligne et Invariants de Sûreté

### 2.1 Pilote Réseau Intel E1000 (`drivers/network/e1000`)

- **Anneaux DMA Circulaires :**
  - Alloue deux anneaux contigus en mémoire physique pour la réception (RX) et la transmission (TX).
  - Chaque descripteur (`RxDesc`, `TxDesc`) fait 16 octets et mappe un tampon physique de 2048 octets.
  - Invariant : Les adresses physiques sont configurées via le contrôleur IOMMU du noyau, interdisant à la carte réseau d'écrire en dehors des tampons RX alloués.
  - Barrière mémoire : Utilise des barrières `core::sync::atomic::fence` entre l'écriture des descripteurs et l'incrément des registres `RDT` / `TDT` (Tail Pointer).

### 2.2 Pilote Stockage NVMe (`drivers/storage/nvme`)

- **Architecture Lock-Free SQ/CQ :**
  - Files de soumission (Submission Queues, SQ) et de complétion (Completion Queues, CQ) circulaires basées sur des pointeurs de tête et queue atomiques.
  - Commandes NVMe de 64 octets (opcode, namespace ID, PRP1, PRP2, LBA, nombre de blocs).
  - Pas d'allocations dynamiques lors du traitement des commandes d'E/S (RÈGLE NO-ALLOC en chemin critique).

### 2.3 Pilote UART Série TTY (`drivers/tty`)

- **Communication Matérielle par Ports I/O :**
  - Contrôle direct des registres 8250 (IER, FCR, LCR, MCR, LSR).
  - Configuration sécurisée du diviseur de baud (115200 bauds, 8N1).
  - Émission bloquante no-alloc pour les traces de diagnostic et panics.

### 2.4 Moteur de Sécurité Verity (`drivers/security/verity`)

- **Source Unique de Vérité Cryptographique :**
  - Partagé à la compilation entre `exo-boot`, le noyau et les outils d'audit.
  - Valide les signatures Ed25519 sur les hachages SHA-512 des segments binaires.
  - Rejette immédiatement toute clé publique corrompue ou signature non concordante.

---

## 3. Synthèse de Robustesse et Isolation

| Pilote | Surface d'Attaque Matérielle | Protection Déployée |
| :--- | :--- | :--- |
| **Intel E1000** | Paquet malformé provoquant un buffer overflow | Bornage strict à 2048 octets par descripteur, isolation IOMMU du domaine réseau. |
| **NVMe** | Commande DMA corrompue écrasant la RAM noyau | Traduction d'adresses IOVA via l'IOMMU, vérification des PRP entries. |
| **VirtIO GPU** | Commande d'affichage invalide | Validation des dimensions du framebuffer virtuel et des coordonnées de scan. |
| **Clavier PS/2** | Injection d'interruptions ou tempête d'IRQ | Filtrage des scancodes et dé-bouncing dans la file `input_server`. |

---

## 4. Statut de Conformité

- **Architecture :** Conforme aux normes PCI 3.0, NVMe 1.4, AHCI 1.3, VirtIO 1.1.
- **Sécurité Mémoire :** Absence d'accès direct mémoire non paginé, respect strict du confinement micro-noyau.
