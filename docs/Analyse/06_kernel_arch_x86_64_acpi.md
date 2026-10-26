# Analyse Approfondie — Module `kernel/src/arch/x86_64/acpi/`

## 1. Vue d'Ensemble du Module

Le sous-module `kernel/src/arch/x86_64/acpi/` implémente un parseur ACPI autonome, minimal et sécurisé, indispensable à la découverte des composants matériels critiques dès le démarrage précoce (*early boot*) :
- **Tables Racines & Découverte** (`parser.rs`) : localisation du pointeur RSDP (*Root System Description Pointer*) dans l'EBDA ou la zone ROM BIOS (ou fourni par le bootloader Multiboot2/UEFI), arbitrage entre RSDT (adresses physiques 32 bits, ACPI 1.0) et XSDT (adresses physiques 64 bits, ACPI 2.0+), et classification des tables filles par leur signature ASCII de 4 octets.
- **Topologie des Interruptions & Cœurs** (`madt.rs`) : parsing de la table MADT/APIC (*Multiple APIC Description Table*) pour extraire les identifiants APIC de tous les cœurs logiques (LAPIC standard et x2APIC), localiser les contrôleurs I/O APIC, et répertorier les redirections de lignes d'interruption ISA vers GSI (*Interrupt Source Overrides*).
- **Horloge HPET** (`hpet.rs`) : initialisation du High Precision Event Timer via l'architecture de fixmap (`FIXMAP_HPET`), mesure de la période en femtosecondes, activation du compteur principal 64 bits et fourniture d'une horloge monotone de référence.
- **Temporisateur PM Timer** (`pm_timer.rs`) : extraction du port d'E/S du Power Management Timer depuis la table FADT (*Fixed ACPI Description Table*), identification de la largeur du registre (24 ou 32 bits) et gestion du débordement cyclique à 3.579545 MHz.

---

## 2. Analyse Fichier par Fichier : Sûreté, Cohérence et Vulnérabilités

### 2.1. `mod.rs`
- **Rôle :** Point d'entrée du sous-module ACPI.
- **Interconnexions :** Déclare et réexporte les interfaces publiques (`hpet`, `madt`, `parser`, `pm_timer`).
- **Jugement de sûreté :** `SAFE`. Aucune logique d'état mutable.

---

### 2.2. `parser.rs`
- **Rôle :** Détection du RSDP et parcours des tables XSDT/RSDT.
- **Interconnexions :**
  - Connecté à `kernel/src/arch/x86_64/boot/multiboot2.rs` pour la récupération de l'adresse RSDP passée par GRUB/bootloader.
  - Connecté à `kernel/src/memory/core/layout.rs` pour la validation des adresses physiques via `acpi_phys_accessible()`.
- **Audit de sûreté & vulnérabilités :**
  - **Défense mémoire physique :** `acpi_phys_accessible(phys, len)` filtre explicitement les adresses inférieures à `0x1000` (prévention des déréférencements nuls) et vérifie que la plage ne dépasse pas `layout::physmap_limit()`, évitant ainsi tout débordement au-delà de la mémoire physique mappée en identité ou couverte par le physmap.
  - **Accès non-alignés sécurisés :** Utilise `acpi_read_unaligned::<T>` plutôt qu'un déréférencement direct de pointeur brut. Cela neutralise tout risque de *Undefined Behavior* (UB) sur les architectures x86_64 strictes et garantit la compatibilité avec Rust 1.82+.
  - **Validation de Checksum :** Le calcul de la somme de contrôle sur 20 octets (pour le RSDP) et sur la totalité de `SdtHeader.length` valide l'intégrité mémoire des tables avant tout traitement.
  - **Borne anti-DoS / Corruption :** Dans `parse_xsdt` et `parse_rsdt`, le nombre d'entrées `n_entries` est borné à 64 et la taille totale à 4096 octets, prévenant toute boucle infinie ou allocation fantôme si une table corrompue présentait une longueur aberrante.
- **Jugement :** `SAFE & ROBUSTE`.

---

### 2.3. `madt.rs`
- **Rôle :** Extraction de la topologie matérielle d'interruption et multiprocesseur.
- **Interconnexions :**
  - Connecté à `kernel/src/arch/x86_64/boot/early_init.rs` pour alimenter le démarrage SMP des coeurs d'application (APs).
  - Connecté à `kernel/src/arch/x86_64/apic/io_apic.rs` via `register_ioapic()`.
  - Connecté à `kernel/src/arch/x86_64/apic/local_apic.rs` via `set_lapic_base()`.
- **Audit de sûreté & vulnérabilités :**
  - **Contrôle des longueurs d'entrées :** Chaque type d'entrée (LAPIC type 0, IOAPIC type 1, ISO type 2, Override type 5, x2APIC type 9) vérifie que `entry_len >= 2 + size_of::<Structure>()` avant de lire les champs correspondants. Si la longueur est corrompue, l'itération s'interrompt immédiatement (`break;`), interdisant tout débordement de tampon hors de la table MADT.
  - **Support x2APIC :** Prend en charge les processeurs modernes disposant d'APIC IDs 32 bits (type 9) avec filtrage des processeurs désactivés (`flags & 3 != 0`).
  - **Contrôle de capacité :** L'enregistrement des APIC IDs dans `info.apic_ids` est strictement borné à 256 (`info.cpu_count < 256`), éliminant tout risque d'écriture hors bornes.
- **Jugement :** `SAFE`. Implémentation conforme à la spécification ACPI 6.4 §5.2.12.

---

### 2.4. `hpet.rs`
- **Rôle :** Gestionnaire MMIO pour le High Precision Event Timer.
- **Interconnexions :**
  - Connecté au sous-système de pagination via la fixmap (`FIXMAP_HPET`) : permet de mapper la page MMIO en mémoire virtuelle sans recourir à l'allocateur de tas noyau, qui n'est pas encore initialisé à cette étape du boot.
  - Connecté à `kernel/src/arch/x86_64/time/sources/hpet.rs` et `kernel/src/arch/x86_64/cpu/tsc.rs`.
- **Audit de sûreté & vulnérabilités :**
  - **Sanity check de la période :** La période d'horloge lue dans `GCAP_ID` est validée dans la plage `100..=100_000_000` femtosecondes (correspondant à une fréquence de 10 MHz à 10 GHz). Toute valeur en dehors de cet intervalle entraîne un rejet immédiat, empêchant une division par zéro lors du calcul de la fréquence.
  - **Séquence de reset du compteur :** Lors de l'initialisation, le compteur est d'abord arrêté (`GEN_CFG &= !HPET_ENABLE`), le registre de comptage remis à zéro (`write_volatile(MAIN_CTR, 0)`), puis réactivé. Cette séquence évite les courses d'interruption matérielle au démarrage.
- **Jugement :** `SAFE`.

---

### 2.5. `pm_timer.rs`
- **Rôle :** Pilote du temporisateur matériel FADT (Power Management Timer).
- **Interconnexions :**
  - Connecté à `kernel/src/arch/x86_64/cpu/tsc.rs` pour la recalibration post-boot.
- **Audit de sûreté & vulnérabilités :**
  - Le port I/O est lu depuis l'offset fixe de la table FADT (+76 octets). Si le port vaut 0 (indiquant l'absence de PM Timer sur certaines plateformes purement UEFI/SoC), la fonction `init_pm_timer()` retourne `false` de manière propre sans tenter d'accès I/O invalide.
  - Le drapeau `PM_TMR_32BIT` est correctement mémorisé d'après le bit 8 du champ `flags` de la FADT (`FADT_TMR_VAL_EXT`), permettant d'appliquer soit le masque 24 bits (`0x00FF_FFFF`), soit le masque 32 bits (`0xFFFF_FFFF`) lors du calcul des deltas de temps écoulé.
- **Jugement :** `SAFE`.

---

## 3. Synthèse des Correctifs & Améliorations Apportés

1. **Remplacement des comparaisons manuelles de plages** par `.contains(&period_fs)` dans `hpet.rs` et `(0x80000..0xA0000).contains(&ebda_base)` dans `parser.rs`.
2. **Protection stricte des accès physiques** via `acpi_phys_accessible()` évitant les fautes de page sur pointeurs corrompus.
