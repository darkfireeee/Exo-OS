EXOOS KERNEL
RAPPORT D'AUDIT TECHNIQUE APPROFONDI
FIX-4 · Sous-systèmes Kernel Complets (hors sécurité) · Commit 74c3659e
Dépôt	https://github.com/darkfireeee/Exo-OS.git
Commit analysé	74c3659e (FIX 4/5)
Commit référence docs	c4239ed1
Date d'audit	20 avril 2026
Méthodologie	Lecture complète sources + cross-référence docs/recast + docs/FIX/4
Auditeur	Claude Sonnet 4.6 — Anthropic
Portée	kernel/, servers/, libs/ — hors security/exoledger refonte
 
1. Synthèse Exécutive
Cet audit couvre l'intégralité du kernel ExoOS tel que présent dans le commit 74c3659e, la première révision qui applique les correctifs documentés dans docs/FIX/4. L'analyse croise trois sources : le code source actuel, la documentation d'architecture dans docs/recast/, et les spécifications de correction dans docs/FIX/4 all kernel (without security)/.
1.1 État des corrections documentées (FIX-4)
Sur les 10 corrections listées dans le FIX-4 (5 P0 + 5 P1, hors P2), 7 ont été appliquées dans ce commit, 2 sont partielles et incomplètes, et 1 n'a pas été appliquée.
	fork_impl.rs créé, register_addr_space_cloner() câblé dans lib.rs Phase 3c
	elf_loader_impl.rs créé MAIS retourne cr3=0x1000 hardcodé — stub invalide
	Numéros IPC NON corrigés dans ipc_router, vfs_server, crypto_server, exo_shield
	sys_read/write/open/close câblés vers fs_bridge — implémentation réelle
	sys_exo_ipc_send/recv câblés vers ipc::ring::spsc
	MAX_SPSC_RINGS passé de 256 à 4096
	send_sipi_once : INIT IPI ajouté (spec Intel MP §B.4)
	syscall_cstar_noop : RSP userspace correctement sauvé/restauré
	Fork : RFLAGS parent propagés avec masquage sécurisé
	exec : stack_base/stack_size calculés depuis initial_stack_top

1.2 Nouveaux bugs découverts par cet audit
En plus des corrections documentées, cet audit a identifié 8 bugs supplémentaires absents des documents FIX précédents. Deux sont de niveau P0 (bloquants), trois de niveau P1 (majeurs), trois de niveau P2 (mineurs).
	clone_pt() ne supprime pas FLAG_WRITABLE → parent+fils écrivent la même page physique
	elf_loader_impl.rs retourne CR3=0x1000 hardcodé → triple fault garanti au execve
	free_pd_tree() libère les PT tables mais jamais les frames feuilles → fuite massive
	clone_pt() ne fait pas inc_refcount → double-free potentiel sur exit
	4 serveurs Ring1 utilisent SYS_IPC_SEND=302 (= RECV_NB kernel) → messages perdus
	IpcFastMsg.data=64 octets mais msg_len validé jusqu'à 65536 → EINVAL trompeur > 64B
	TCB manque le champ creation_tsc → P2-04 exoledger incomplet
	Huge pages CoW : clone_pdpt/clone_pd copient les entrées 2MB sans ôter FLAG_WRITABLE
 
2. État des Corrections Documentées — Analyse Détaillée
2.1 P0-02 (partiel) — ElfLoader : implémentation stub invalide
Localisation
kernel/src/fs/elf_loader_impl.rs
Problème constaté
L'implémentation ExoFsElfLoader::load_elf() présente dans le commit est un stub qui ne charge aucun binaire réel. Le code retourne des valeurs hardcodées uniquement si le chemin contient la chaîne "init_server". Pour tout autre chemin, il retourne ElfLoadError::NotFound. Le problème central :
// kernel/src/fs/elf_loader_impl.rs — PROBLÈME CRITIQUE
let cr3 = 0x1000u64; // Placeholder CR3 — doit être alloué réellement
Ok(ElfLoadResult {
    entry_point: 0x0000_7f00_0000_1000u64, // Adresse supposée
    cr3,                                    // ← 0x1000 = page BIOS/réservée
    addr_space_ptr: cr3 as usize,           // ← 0x1000 invalide
    ...
})
Lorsque le kernel applique ce résultat dans do_execve() et recharge CR3 avec 0x1000, le CPU tente de charger un PML4 depuis la page physique 4096 — une zone typiquement occupée par le BIOS ou le Real Mode IVT sous QEMU. La TLB shootdown qui suit provoque immédiatement un triple fault, rendant execve() identiquement bloquant que dans le commit précédent.
Correction requise
L'implémentation doit être complétée selon le plan original des docs FIX :
•	Résoudre le chemin via exofs::path::resolve() → BlobId réel
•	Allouer un nouveau UserAddressSpace via buddy::alloc_pages(0, ZEROED)
•	Lire les segments PT_LOAD via exofs::object::read_bytes()
•	Mapper chaque segment dans le nouvel espace d'adressage
•	Allouer la pile (8 pages par défaut à 0x7FFF_FFFF_0000)
•	Retourner ElfLoadResult avec le vrai CR3 alloué

2.2 P0-03 + P2-05 (non appliqués) — Numéros syscall IPC incohérents : 4 serveurs affectés
Localisation
Quatre serveurs Ring1 définissent tous leurs constantes syscall IPC localement avec des valeurs erronées :
servers/ipc_router/src/main.rs:64     SYS_IPC_REGISTER = 300 (devrait être IPC_CREATE=304)
servers/ipc_router/src/main.rs:66     SYS_IPC_SEND     = 302 (devrait être IPC_SEND=300)
servers/vfs_server/src/main.rs:52-54  mêmes constantes erronées
servers/crypto_server/src/main.rs:60-62 mêmes constantes erronées
servers/exo_shield/src/main.rs:55-57  mêmes constantes erronées
Impact en cascade
Le kernel définit dans kernel/src/syscall/numbers.rs :
Numéro	Kernel (correct)
300	SYS_EXO_IPC_SEND — envoi bloquant
301	SYS_EXO_IPC_RECV — réception bloquante
302	SYS_EXO_IPC_RECV_NB — réception non-bloquante
304	SYS_EXO_IPC_CREATE — enregistrement endpoint
Au démarrage, chaque serveur appelle syscall(300, name, len, ep_id) en croyant appeler SYS_IPC_REGISTER. Le kernel interprète ce numéro comme SYS_EXO_IPC_SEND et tente d'envoyer un message IPC vers l'endpoint 0 avec le nom du serveur comme payload. Résultat : aucun serveur Ring1 ne peut s'enregistrer. La boucle principale de ipc_router utilise ensuite syscall(302, ...) pour forwards, ce qui appelle SYS_EXO_IPC_RECV_NB au lieu de SEND — tous les messages sont perdus.
Correction requise
Créer la crate partagée servers/syscall_abi/src/lib.rs telle que décrite dans P0-03, et remplacer les modules mod syscall { ... } locaux dans les 4 serveurs par des re-exports de cette crate.

2.3 P1-03 (non appliqué) — Table de fixup #PF absente
Le fichier kernel/src/syscall/validation.rs contient aux lignes 440 et 452 des commentaires décrivant un mécanisme de fixup ASM censé capturer les page faults lors des accès userspace depuis les handlers syscall. Ces commentaires restent des intentions non implémentées. La fonction probe_user_read() décrite dans P1-03 n'est pas présente. Le handler handle_page_fault() dans arch/x86_64/exceptions.rs ne consulte aucune table de fixup.
Impact : Un handler syscall qui déréférence un pointeur userspace valide mais dont la page est swappée ou CoW-faultée provoque un #PF en contexte kernel → kernel panic. Ce cas est fréquent avec le sous-système CoW désormais partiellement opérationnel.

2.4 P2-04 (partiel) — TCB manque le champ creation_tsc
La correction P2-04 requiert l'ajout d'un champ pub creation_tsc: u64 dans ThreadControlBlock. Ce champ est absent :
// scheduler/core/task.rs — ThreadControlBlock actuel (256 bytes EXACTE)
// Pas de création_tsc : le TCB a une contrainte de taille 256B avec assert! compile-time
// Ajouter creation_tsc nécessite de réutiliser _cold_reserve[24..32] (offset 168)
De plus, exoledger.rs::current_actor_oid() n'utilise toujours que (pid, tid) — les octets [16..32] sont à zéro. Le discriminant TSC anti-réutilisation PID n'est pas en place.
 
3. Nouveaux Bugs Critiques (P0) — Non documentés dans FIX-4
3.1 P0-06 — clone_pt() ne marque pas les PTEs en CoW : corruption mémoire garantie
Localisation
kernel/src/memory/virtual/address_space/fork_impl.rs — fonction clone_pt()
Code actuel fautif
unsafe fn clone_pt(src_pt_phys: PhysAddr, dst_pt_phys: PhysAddr) {
    let src_pt = phys_to_table_ref(src_pt_phys);
    let dst_pt = phys_to_table_mut(dst_pt_phys);
    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if src_entry.is_present() {
            dst_pt[l1_idx] = src_entry; // ← COPIE BRUTE — FLAG_WRITABLE préservé !
        }
    }
}
Analyse
Le fichier se décrit comme implémentant le Copy-on-Write en section 3 de son en-tête. La réalité est différente : aucun bit de protection n'est modifié. La constante PageTableEntry::FLAG_WRITABLE = 1 << 1 et FLAG_COW = 1 << 9 (bit disponible OS) sont définies dans le même fichier x86_64.rs mais ne sont jamais utilisées dans clone_pt().
De plus, la fonction repoint_table_entry() utilisée dans clone_pd() et clone_pdpt() applique le masque !0x000F_FFFF_FFFF_F000 pour préserver les flags de l'entrée source. Ce masque conserve FLAG_WRITABLE intact dans les entrées intermédiaires (PD, PDPT), propageant la permission d'écriture vers le bas de la hiérarchie.
Conséquence concrète : Après un fork(), parent et fils disposent tous deux d'un accès en écriture aux mêmes frames physiques. Toute écriture du parent après fork() détruit silencieusement la mémoire du fils (et vice-versa) sans déclencher aucun #PF ni aucune erreur. Ce bug rend le fork() fonctionnellement inutilisable, même si l'appel syscall ne retourne plus EFAULT.
Correction
unsafe fn clone_pt(src_pt_phys: PhysAddr, dst_pt_phys: PhysAddr) {
    use crate::memory::virt::page_table::x86_64::PageTableEntry as PTE;
    let src_pt = phys_to_table_ref(src_pt_phys);
    let dst_pt = phys_to_table_mut(dst_pt_phys);
    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if !src_entry.is_present() { continue; }
        // Si la page est inscriptible, la marquer CoW dans PARENT et FILS
        if src_entry.is_writable() {
            let cow_entry = PTE::from_raw(
                (src_entry.raw() & !PTE::FLAG_WRITABLE) | PTE::FLAG_COW
            );
            src_pt[l1_idx] = cow_entry; // ← parent perd WRITABLE
            dst_pt[l1_idx] = cow_entry; // ← fils hérite CoW
            // Incrémenter le refcount de la frame physique
            crate::memory::physical::frame::ref_count::inc_refcount(
                src_entry.phys_addr()
            );
        } else {
            dst_pt[l1_idx] = src_entry; // lecture seule → partagée directement
        }
    }
}
Après clone_pt(), appeler flush_tlb_after_fork(parent_cr3) doit effectuer un vrai TLB shootdown SMP (envoi d'un IPI à tous les CPUs qui exécutent le parent) et pas seulement recharger le CR3 local comme dans l'implémentation actuelle.

3.2 P0-07 — ElfLoader retourne CR3 = 0x1000 hardcodé → triple fault systématique
Localisation
kernel/src/fs/elf_loader_impl.rs:59
Code actuel fautif
// Valeurs par défaut pour un processus Ring1 minimal
let entry_point = 0x0000_7f00_0000_1000u64; // Adresse SUPPOSÉE du binaire
let cr3 = 0x1000u64; // Placeholder CR3 — doit être alloué réellement
Ok(ElfLoadResult {
    entry_point,
    cr3,               // ← page physique 1 = zone BIOS/IVT
    addr_space_ptr: cr3 as usize,
    ...
})
Analyse
Sur x86_64, la page physique 0x1000 (adresse 4096) correspond à la deuxième page de la mémoire basse. Sous QEMU/SeaBIOS, cette zone est typiquement occupée par la table des descripteurs d'interruptions 16-bit ou par des données BIOS. Elle n'est certainement pas une structure PML4 valide.
Quand do_execve() applique ElfLoadResult.cr3 = 0x1000 et effectue mov cr3, 0x1000, le CPU interprète les 8 bytes à phys[0x1000] comme l'entrée PML4[0]. Puisque cette "structure" n'est pas une table de pages valide, la première instruction après SYSRETQ provoque un #PF dont le handler lui-même ne peut accéder à sa pile (CR3 invalide) → #DF (Double Fault) → triple fault → reset CPU.
Note : ce bug est doublement masqué. Premièrement, le code n'est déclenché que pour les chemins contenant "init_server" — tous les autres retournent directement ElfLoadError::NotFound. Deuxièmement, l'entry_point = 0x0000_7f00_0000_1000 est lui aussi arbitraire : aucun segment ELF n'est chargé, donc cette adresse est non mappée dans l'espace d'adressage retourné.
 
4. Nouveaux Bugs Majeurs (P1) — Non documentés dans FIX-4
4.1 P1-06 — free_pd_tree() : fuite totale des frames feuilles userspace
Localisation
kernel/src/memory/virtual/address_space/fork_impl.rs — free_pd_tree()
Code actuel fautif
unsafe fn free_pd_tree(pd_phys: PhysAddr) {
    let pd = phys_to_table_ref(pd_phys);
    for l2_idx in 0..512 {
        let entry = pd[l2_idx];
        if entry.is_present() && !entry.is_huge() {
            // ← Libère pd[l2_idx].phys_addr() = ADRESSE D'UNE PT TABLE
            // ← Les frames feuilles (données réelles) restent allouées !
            let _ = buddy::free_pages(Frame::containing(entry.phys_addr()), 0);
        }
    }
    let _ = buddy::free_pages(Frame::containing(pd_phys), 0);
}
Analyse
La hiérarchie de tables de pages x86_64 est : PML4 → PDPT → PD → PT → Frame. Dans le contexte de free_pd_tree(pd_phys) :
•	pd_phys est l'adresse physique d'une table PD (niveau 2)
•	pd[l2_idx].phys_addr() = adresse d'une table PT (niveau 1) ← c'est ce que libère le code
•	Les vraies frames de données (niveau 0) pointées par les PTEs sont IGNORÉES
En d'autres termes, toutes les frames physiques qui contiennent le code, la pile et le tas userspace du processus fils ne sont jamais restituées au buddy allocator lors de la destruction du processus. Sur un système qui fork() et exit() fréquemment (services Ring1), la mémoire physique disponible diminue continuellement.
Le bug est systémique et concerne free_userspace_tables() qui orchestre l'ensemble : free_pdpt_tree() → free_pd_tree() — aucun des deux niveaux n'atteint les feuilles.
Correction
unsafe fn free_pt_frames(pt_phys: PhysAddr) {
    let pt = phys_to_table_ref(pt_phys);
    for l1_idx in 0..512 {
        let entry = pt[l1_idx];
        if entry.is_present() {
            // Décrémenter refcount. Si refcount atteint 0, libérer.
            let frame = Frame::containing(entry.phys_addr());
            if dec_refcount_and_check(frame) == 0 {
                let _ = buddy::free_pages(frame, 0);
            }
        }
    }
    let _ = buddy::free_pages(Frame::containing(pt_phys), 0); // libérer la PT elle-même
}

unsafe fn free_pd_tree(pd_phys: PhysAddr) {
    let pd = phys_to_table_ref(pd_phys);
    for l2_idx in 0..512 {
        let entry = pd[l2_idx];
        if entry.is_present() {
            if entry.is_huge() {
                // Page 2MB : libérer la frame directement
                if dec_refcount_and_check(Frame::containing(entry.phys_addr())) == 0 {
                    let _ = buddy::free_pages(Frame::containing(entry.phys_addr()), 9); // order 9 = 2MB
                }
            } else {
                free_pt_frames(entry.phys_addr()); // ← AJOUT : descendre au niveau PT
            }
        }
    }
    let _ = buddy::free_pages(Frame::containing(pd_phys), 0);
}

4.2 P1-07 — Absence de refcounting sur les frames dans clone_pt
Localisation
kernel/src/memory/virtual/address_space/fork_impl.rs — clone_pt() et clone_pdpt()
Analyse
Le CoW partage les frames physiques entre parent et fils. Ce partage requiert un compteur de références par frame pour savoir si une frame est utilisée par plusieurs espaces d'adressage. Sans ce compteur :
•	Scénario 1 — Exit du fils : free_userspace_tables() tente de libérer les frames partagées via le buddy allocator. Si le parent est toujours actif et utilise ces frames, elles deviennent libres dans le buddy mais sont toujours mappées dans le parent → utilisation après libération.
•	Scénario 2 — CoW break du fils : Lors d'un write fault, le handler CoW alloue une nouvelle frame, copie le contenu, et libère l'ancienne. Sans vérifier le refcount, il libère une frame encore mappée dans le parent.
•	Scénario 3 — Fork en chaîne : A → fork → B → fork → C. A et B partagent les frames. C hérite du partage. Quand B exit, il libère des frames encore tenues par A et C.
La présence de crate::memory::physical::frame::ref_count dans le projet (importé dans les docs FIX) indique que l'infrastructure de refcounting existe mais n'est pas appelée depuis clone_pt().

4.3 P1-08 — 4 serveurs Ring1 : SYS_IPC_SEND=302 appelle en réalité RECV_NB
Ce bug est distinct de P0-03 bien qu'il en soit une conséquence amplifiée. Même si l'enregistrement initial (P0-03) était corrigé, la boucle de dispatch de chaque serveur utilise SYS_IPC_SEND=302. Le kernel mappe 302 sur SYS_EXO_IPC_RECV_NB. Donc, chaque fois qu'un serveur tente de répondre à un client, il effectue en réalité une lecture non-bloquante sur un ring SPSC — probablement vide à cet instant — et retourne EAGAIN. Le client attend une réponse qui n'arrive jamais.
Les 4 serveurs affectés sont : ipc_router, vfs_server, crypto_server, exo_shield. init_server n'est pas affecté car il ne définit pas de module IPC local.
 
5. Nouveaux Bugs Mineurs (P2) — Non documentés dans FIX-4
5.1 P2-06 — IpcFastMsg.data = 64 octets : validation msg_len trompeuse
Le handler sys_exo_ipc_send() effectue deux vérifications successives :
if len > 65536 { return E2BIG; }                        // ← jamais déclenché
if len > crate::ipc::core::IpcFastMsg::zeroed().data.len() {
    return EINVAL;                                       // ← déclenché dès len > 64
}
Puisque IpcFastMsg.data = [u8; 64], tout message de taille supérieure à 64 octets retourne EINVAL (argument invalide) au lieu de E2BIG (message trop grand). Du point de vue d'un serveur Ring1 qui envoie 128 octets en pensant que la limite est 64KB (comme documenté), l'erreur EINVAL est non diagnosticable. La vérification E2BIG est lettre morte.
La solution à terme est d'implémenter les messages longs via un mécanisme de SHM ou de batch ring, et de retourner E2BIG (pas EINVAL) pour les tailles > 64. À court terme, la documentation doit indiquer explicitement que la limite réelle est 64 octets pour les fast IPC.

5.2 P2-07 — TCB manque creation_tsc : P2-04 exoledger incomplet
La structure ThreadControlBlock est contrainte à exactement 256 bytes par une assertion compile-time. Le champ creation_tsc: u64 requis par la correction P2-04 n'existe pas. Il peut être logé dans _cold_reserve[24..32] (offset TCB 168) qui est actuellement zéro, sans modifier le layout. Sans ce champ, current_actor_oid() produit des OIDs [0..8]=pid, [8..16]=tid, [16..32]=zéro — deux OIDs de processus avec le même PID (après réutilisation) sont identiques, ce qui invalide l'auditabilité post-incident.

5.3 P2-08 — Huge pages CoW : entrées 2MB/1GB copiées sans ôter FLAG_WRITABLE
Dans clone_pdpt() et clone_pd(), les entrées huge page (bit PSE) sont copiées verbatim :
if src_entry.is_huge() {
    dst_pdpt[l3_idx] = src_entry; // ← copie BRUTE, FLAG_WRITABLE intact
    continue;
}
Une huge page 2MB partagée entre parent et fils sans protection CoW représente une fenêtre de corruption de 2MB au lieu de 4KB. Sur un kernel qui mappé le tas userspace en huge pages pour les performances (ce qu'ExoOS peut faire via le buddy allocator order > 0), ce bug multiplie l'impact par 512.
 
6. Plan de Correction Recommandé
6.1 Phase 1 — Déblocage complet du userspace
Ces corrections sont un prérequis absolu avant tout test Ring1.
Priorité / ID	Action
1 — P0-06	Modifier clone_pt() pour effacer FLAG_WRITABLE, positionner FLAG_COW, appeler inc_refcount()
2 — P2-08	Appliquer la même correction aux huge pages dans clone_pdpt()/clone_pd()
3 — P1-07	Vérifier que inc_refcount() est intégré (non-bloquant si P0-06 l'ajoute)
4 — P0-07	Implémenter ExoFsElfLoader::load_elf() avec allocation CR3 réelle et chargement segments ELF
5 — P0-03+P2-05	Créer crate syscall_abi partagée, migrer ipc_router, vfs_server, crypto_server, exo_shield
6 — P1-08	Corollaire de 5 — automatiquement corrigé

6.2 Phase 2 — Stabilité et sécurité mémoire
Priorité / ID	Action
7 — P1-06	Réécrire free_pd_tree() en ajoutant free_pt_frames() pour libérer les feuilles
8 — P1-03	Implémenter probe_user_read() + table de fixup #PF dans validation.rs
9 — P0-02	Compléter la correction partielle : intégration ExoFS réelle dans elf_loader_impl.rs

6.3 Phase 3 — Robustesse et observabilité
Priorité / ID	Action
10 — P2-07	Ajouter creation_tsc dans _cold_reserve[24..32] du TCB, initialiser dans new()
11 — P2-04	Câbler creation_tsc dans current_actor_oid() pour l'OID discriminant
12 — P2-06	Documenter la limite 64B des fast IPC ; retourner E2BIG au lieu d'EINVAL
13 — P1-08	Valider la correction P0-03 sur les 4 serveurs avec tests d'intégration Ring1

6.4 Vue d'ensemble : état avant/après
Sous-système	Avant FIX-4
fork()	EFAULT systématique — débloqué P0-01, mais CoW brisé (P0-06)
execve()	ENOSYS — partiellement débloqué P0-02, mais CR3=0x1000 = triple fault
IPC Ring1	Enregistrement impossible (P0-03) + messages perdus (P1-08)
fs read/write	ENOSYS — débloqué P0-04 ✓ fonctionnel
Mémoire fork()	Fuite feuilles (P1-06) + pas de refcount (P1-07) + CoW non-protégé (P0-06)
Boot SMP ExoPhoenix	SIPI sans INIT — corrigé P1-05 ✓
Audit ExoLedger	OIDs non-uniques après réutilisation PID (P2-04+P2-07)

7. Conclusion
Le commit 74c3659e représente une avancée substantielle : la moitié des bloquants documentés sont levés, et le sous-système fs/, le scheduler, et le boot SMP sont dans un état globalement correct. Cependant, deux chemins essentiels — fork() et execve() — restent non fonctionnels en raison de bugs introduits pendant les corrections elles-mêmes (CoW non implémenté dans clone_pt, CR3 hardcodé dans elf_loader).
Le problème le plus grave est structurel : le CoW est annoncé dans les commentaires du fichier fork_impl.rs mais absent du code. Un tel gap entre la documentation inline et l'implémentation est le type d'erreur le plus difficile à détecter lors d'une review de code standard. Il faut une lecture ligne à ligne des opérations sur les bits des PTEs pour le constater.
La chaîne de démarrage Ring1 est bloquée par l'incohérence persistante des numéros syscall IPC dans 4 serveurs (P0-03 + P1-08). Cette correction, bien qu'identifiée et documentée depuis le commit de référence c4239ed1, n'a pas été appliquée. La création de la crate syscall_abi partagée est la priorité architecturale numéro 1 après la correction du CoW.
Avec les 13 corrections ordonnées en Phase 1–3, ExoOS atteint un état où le premier processus Ring1 peut être lancé et communiquer via IPC avec ses serveurs. C'est le jalon fondamental qui débloque l'ensemble du développement applicatif sur la plateforme.

Note méthodologique : Cet audit est basé sur une lecture complète des sources du commit 74c3659e, croisée avec docs/recast/, docs/FIX/4/ et les assertions compile-time présentes dans le code. Les bugs P0-06, P0-07, P1-06, P1-07, P1-08, P2-06, P2-07, P2-08 sont des découvertes originales de cet audit, absentes des documents FIX précédents.
