# Analyse Exhaustive Ligne par Ligne — `kernel/src/memory/swap/`

## 1. Vue d'Ensemble du Sous-Système Swap & Zswap

Le sous-système d'éviction et de mémoire virtuelle secondaire (`kernel/src/memory/swap/`) gère le transfert transparent des pages physiques froides vers des supports de stockage secondaires ou vers un cache compressé en RAM (zswap). Il permet de maintenir le système fonctionnel lors des pics de pression mémoire sans déclencher brutalement l'Out-Of-Memory Killer (OOMK).

Le module se structure en 4 volets :
- `compress.rs` : Cache de compression en mémoire vive (zswap pool), compactant les pages 4 KiB en slots de taille maximale `ZSWAP_SLOT_SIZE = 2048` octets avec contrôle d'intégrité par checksum XOR-fold 16 bits.
- `cluster.rs` : Regroupement de pages swap en clusters contigus pour maximiser le débit d'E/S sur les périphériques de blocs (réduction des opérations de recherche / seeks).
- `backend.rs` : Abstraction de périphériques de stockage de swap (disques virtio-blk, NVMe, AHCI ou partition brute).
- `policy.rs` : Stratégies de sélection et de remplacement de pages (algorithme LRU - Least Recently Used approché avec vieillissement d'accès).

---

## 2. Analyse Ligne par Ligne et Invariants de Sécurité

### 2.1 Cache Mémoire Compressé Zswap (`compress.rs`)
- **Seuil d'Éligibilité et Règle de Compression :**
  - Une page candidate n'est conservée dans le pool zswap que si son taux de compression permet de la stocker dans un tampon $\le 2048$ octets (gain $\ge 50\%$). Au-delà, elle est marquée `NotCompressible` et dirigée vers le backend de stockage permanent.
  - Calcul de redondance : `xor_checksum` utilise l'opérateur d'assignation bit à bit compact `acc ^= b as u16` combiné à une rotation de bits vers la gauche (`rotate_left(5)`).
- **Gestion des Emplacements (`ZswapSlot`) :**
  - Tableau statique `ZSWAP_SLOTS` sécurisé par initialisation constante inline `[const { ZswapSlot::new() }; MAX_ZSWAP_SLOTS]`, évitant toute instanciation intermédiaire de constante mutable.
  - La sérialisation des accès mutuels à `zswap_slots_mut()` est formellement garantie par le verrouillage du `Mutex` de `ZswapPool`.

### 2.2 Clustérisation et Allocation Linéaire (`cluster.rs`)
- Allocation par grappes de 64 pages contiguës pour optimiser les écritures par blocs d'E/S asynchrones DMA.

### 2.3 Politique de Remplacement (`policy.rs`)
- Gestion du ratio de swappiness et arbitrage entre éviction de cache de fichiers (page cache) et pagination de mémoire anonyme.

---

## 3. Synthèse des Corrections Appliquées

| Fichier | Problème Détecté | Correction Appliquée |
| :--- | :--- | :--- |
| `compress.rs` | Assignation XOR non concise (`acc = acc ^ (...)`) | Remplacement par `acc ^= b as u16` |
| `compress.rs` | `declare_interior_mutable_const` sur `ZSWAP_SLOTS` | Remplacement par constante inline `[const { ZswapSlot::new() }; ...]` |
| `compress.rs` | Absence de `Default` sur `ZswapPool` | `impl Default for ZswapPool` |

---

## 4. Statut de Validation

Le sous-système `kernel/src/memory/swap/` est vérifié, conforme aux contraintes de concurrence et exempt de tout avertissement compilateur.
