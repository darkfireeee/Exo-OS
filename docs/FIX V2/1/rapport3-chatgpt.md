Audit de Sécurité — Troisième Passe Approfondie (modules noyau, exo_shield, crypto_server et ExoPhoenix)
Introduction

Ce rapport constitue une troisième passe d’audit « à froid » des composants de sécurité d’Exo‑OS. Il se concentre sur :

Le noyau de sécurité (kernel/src/security/*) : analyse des vulnérabilités et écarts entre code et spécifications.
Le serveur ExoShield (servers/exo_shield) : conformité à la spécification et séparation des responsabilités.
Le serveur crypto_server (servers/crypto_server) : centralisation des primitives cryptographiques et gestion du keystore.
Le module ExoPhoenix (gel/restauration) : cohérence entre spécifications (ExoPhoenix_Spec_v7) et implémentation.

Les conclusions s’appuient sur la documentation officielle (spécifications recast et TLA+), les rapports d’audit précédents et l’inspection du code source. Chaque écart relevé est justifié par des citations précises.

1. Sécurité du noyau
1.1 Race condition au démarrage (CVE‑EXO‑001)
Observation : Le drapeau SECURITY_READY informe les processeurs secondaires (AP) que l’initialisation sécurité est terminée. La documentation indique que les AP doivent attendre ce drapeau avant d’effectuer des IPC. Cependant, dans arch/x86_64/smp/init.rs, la routine ap_entry() n’implémente aucun spin_loop() : les AP rejoignent le scheduler sans attendre la levée de SECURITY_READY. 
Conséquence : pendant quelques millisecondes, un AP peut exécuter des syscalls ou des IPC sans que les capacités de contrôle d’accès ne soient initialisées, violant l’invariant « ¬SecurityReady ⟹ ¬IPC » de la spécification ExoShield.
Recommandation : appliquer la correction du patch 01 : appeler ap_wait_security_ready() (ou un spin‑wait explicite) dans ap_entry() immédiatement après l’initialisation du FPU.
1.2 Vérification des capacités non constant‑time
Observation : la fonction capability::verify() suit le principe CAP‑05 en effectuant les mêmes opérations pour un token valide ou invalide. Toutefois, elle utilise des comparaisons Rust classiques (==, contains()) et ne fait appel à aucune fonction de comparaison constant‑time. 
Risque : des attaques par canal auxiliaire peuvent mesurer les différences de timing et inférer l’existence de droits ou de génération de token.
Recommandation : importer la crate subtle et remplacer les comparaisons par ConstantTimeEq/Choice, comme proposé dans le patch 07 (section 1).
1.3 Absence de vérification de la taille du TCB
Observation : la spécification GI‑01 impose que le TCB (Thread Control Block) fasse exactement 256 octets et réserve des offsets précis pour le token de shadow stack et les drapeaux CET. Or, aucun static_assert! ne garantit cette taille dans task.rs.
Risque : une future extension du TCB pourrait décaler des champs critiques (offset 144 et 152), entraînant des corruptions silencieuses lors de l’écriture WRSSQ et compromettant la protection CET.
Recommandation : ajouter une assertion compile‑time dans task.rs ou utiliser const_assert pour garantir que size_of::<ThreadControlBlock>() == 256.
1.4 Fonction KPTI non implémentée (mark_a_pages_not_present)
Observation : lors de la séquence ExoPhoenix, la fonction mark_a_pages_not_present() dans kernel/src/exophoenix/isolate.rs est laissée vide et annotée TODO. Elle devrait invalider les mappages de Kernel A pour que Kernel B ne puisse plus accéder à ces pages après le handoff.
Risque : un accès mémoire malveillant ou accidentel par le kernel restauré pourrait lire ou modifier les pages de Kernel A, brisant l’isolation attendue.
Recommandation : implémenter la fonction conformément au patch 07 (section 2) afin de défaire les entrées de TLB et de marquer les pages comme non présentes.
1.5 Validation du TCB dans pmc_snapshot()
Observation : la fonction pmc_snapshot() lit les compteurs de performance sans vérifier que le TCB fourni appartient au thread courant. Les patchs recommandent de comparer l’identifiant de processus du TCB avec le thread courant et de vérifier les droits PMC_READ.
Risque : un processus peut observer les compteurs PMC d’un autre processus, divulguant des informations sensibles (canal auxiliaire inter‑processus).
Recommandation : implémenter la validation du TCB et des capabilities dans pmc_snapshot() pour respecter l’isolation et la ségrégation des droits.
1.6 Vulnérabilités supplémentaires et incohérences

Les rapports d’analyse listent plusieurs vulnérabilités supplémentaires :

CET per‑thread absent (CVE‑EXO‑002) : initialement, enable_cet_for_thread() n’était jamais appelé, désactivant le shadow stack. Les correctifs appliqués après l’audit montrent que l’appel est désormais présent dans kernel/src/security/mod.rs et lors de la création de threads.
verify_p0_fixes() manquant (CVE‑EXO‑003) : l’absence de cette vérification dans le boot a été corrigée et l’appel est désormais visible dans exoseal_boot_phase0() et exoseal_boot_complete().
cap_deadline_table sans PKS (CVE‑EXO‑004) : la table des deadlines est exposée sans protection PKS pendant l’initialisation, ce qui reste partiellement corrigé.
Watchdog NMI : le timeout du watchdog est codé en dur à 500 ms, entraînant des faux positifs en environnement virtualisé.
IOMMU NIC policy et ExoCordon : la spécification exige une politique DMA statique pour le NIC et un graphe d’autorité IPC, mais ces mécanismes ne sont pas implémentés dans le code actuel.
Chaînage ExoLedger incomplet : la spec demande un chaînage Blake3 pour chaque entrée du journal, mais l’audit signale que ce point n’est pas systématiquement vérifié.

L’ensemble de ces points doit être intégré à la feuille de route de correction, qui priorise la race condition au démarrage, la mise en place d’un constant‑time pour les comparaisons de capacités et l’ajout d’asserts statiques.

2. Serveur ExoShield (PID 10)
2.1 Différence de périmètre avec ExoShield v1.0
Observation : la spécification ExoShield_v1_Production.md décrit une couche de sécurité de boot (ExoSeal, ExoCage, ExoVeil, ExoLedger, ExoKairos, etc.) et un orchestrateur léger. En revanche, le répertoire servers/exo_shield est un service Ring 1 de confinement applicatif et de surveillance de processus. Les deux composants partagent le nom « ExoShield » mais n’ont ni les mêmes responsabilités ni le même périmètre.
Conséquence : cette homonymie entretient une confusion ; certaines personnes pensent que le serveur Ring 1 implémente la politique hardware du boot, alors qu’il s’agit d’un moteur de scan indépendant. Une clarification est nécessaire pour éviter toute sur‑interprétation des garanties du serveur.
Recommandation : maintenir la séparation documentaire : ExoShield_v1_Production.md reste la source de vérité pour la couche kernel, tandis que ExoShield_Server_v1.md détaille le protocole et les responsabilités du serveur Ring 1. Une note explicite doit être ajoutée dans la documentation pour rappeler cette distinction, et le nom du package Cargo doit être corrigé (exo-exo-shield → exo-shield).
2.2 Cryptographie locale et violation de SRV‑02
Observation : malgré la règle SRV‑02 qui impose de déléguer toutes les opérations cryptographiques au crypto_server, le module signatures/update.rs du serveur ExoShield contient encore une implémentation locale d’Ed25519 et un hash simplifié. Le code indique lui‑même que ce bloc est provisoire et devrait être remplacé par un appel au crypto_server.
Risque : en contournant la centralisation, ce module réintroduit du code cryptographique en Ring 1, susceptible de divergences ou de vulnérabilités par rapport à l’implémentation centralisée. 
Recommandation : supprimer l’implémentation locale et déléguer explicitement la vérification de signatures à crypto_server. Le document ExoShield_Server_v1.md rappelle que les vérifications de signatures doivent passer par ce service.
2.3 Manque d’alignement avec les modèles TLA
Observation : les modèles TLA ExoShield.tla et ExoShield_v1.tla modélisent des invariants matériels et des politiques de handoff (watchdog, IOMMU NIC, DAG d’autorité IPC). Le serveur ExoShield, quant à lui, implémente un moteur de détection heuristique, un sandbox et un module forensics. Il n’existe pas de modèle formel pour ce serveur.
Conséquence : la preuve formelle ne s’applique qu’à la couche kernel, laissant une zone grise pour les garanties du serveur applicatif. 
Recommandation : rédiger un modèle formel ou au minimum une documentation plus précise des invariants du serveur ExoShield afin de pouvoir raisonner sur sa sécurité.
2.4 Problèmes hérités des serveurs (permis et timeouts)

L’analyse transversale des modules servers/ a relevé plusieurs problèmes qui s’appliquent également à exo_shield :

Dépendances et nommage : le package Cargo est nommé exo-exo-shield, ce qui duplique le préfixe exo et contrevient aux conventions de nommage. Une correction simple est proposée.
Incohérences PID/endpoint : certains serveurs utilisent des enregistrements implicites alors que exo_shield utilise explicitement l’endpoint 10. Le risque est un routage ambigu si d’autres serveurs s’enregistrent sans identifiant explicite.
Timeouts IPC : la plupart des serveurs (dont exo_shield) définissent un IPC_RECV_TIMEOUT_MS à 5 000 ms, mais network_server, memory_server, scheduler_server et device_server n’ont aucun timeout, ce qui peut entraîner un blocage indéfini.
Vérification des permissions : dans plusieurs handlers des autres serveurs, les fonctions ne vérifient pas systématiquement sender_pid, permettant à n’importe quel processus de déclencher des actions sensibles. Bien que l’example provienne de device_server, la recommandation de systématiser ces vérifications s’applique aussi à exo_shield.
Keystore fixe du crypto_server : le crypto_server dispose d’un magasin de clés limité à 32 slots sans mécanisme d’éviction. Un processus malveillant peut monopoliser les slots et provoquer un déni de service. Un quota par PID et un système d’expiration sont recommandés.
3. Serveur Crypto (PID 4)
3.1 Vérification de capability CAP‑01
Observation : la documentation évoque un CAP‑01 imposant de vérifier un token de capability avant toute opération. Dans le code actuel, l’appel à exo_cap_check() est réalisé par la fonction authorize_request() qui vérifie la présence d’un cap_token non vide, appelle exo_cap_check() et s’assure que l’objet non nul est valable avant de poursuivre l’exécution. Le code renvoie une erreur si le check échoue. Par conséquent, le principe CAP‑01 est effectivement appliqué pour les requêtes externes.
Clarification : le rapport d’audit du 30 avril 2026 mentionnait qu’aucun appel de vérification de capability n’était visible dans le fichier examiné. Cette critique semble refléter un état antérieur ; la version actuelle comporte un contrôle explicite via exo_cap_check(), ce qui ferme cette vulnérabilité.
3.2 Reseed post‑Phoenix manquant
Observation : la spécification GI-05_ExoPhoenix.md et la correction CORR‑12 imposent qu’au retour d’un gel ExoPhoenix, le noyau envoie un message PhoenixWakeEntropy au crypto_server, lequel doit réinitialiser son compteur de nonces afin d’éviter la réutilisation de nonces. Le rapport d’audit du 30 avril note qu’aucun appel PhoenixWakeEntropy ou phoenix_reseed() n’est présent dans le code examiné.
Conséquence : si des données sont chiffrées entre le gel et le crash, puis que la RAM est restaurée sans reseed, le crypto_server réutilise les mêmes nonces, compromettant la confidentialité XChaCha20 et Poly1305.
Recommandation : implémenter la séquence phoenix_reseed() décrite dans les corrections CORR‑12 et intégrer l’outil PhoenixWakeEntropy dans l’API IPC, avec un accusé de réception et un timeout contrôlé.
3.3 Gestion du magasin de clés (keystore)
Observation : le crypto_server utilise un tableau de 32 ou 64 slots pour stocker les clés (constante MAX_KEYS). Lorsqu’aucun slot n’est disponible, la fonction ks_insert renvoie zéro sans erreur explicite, ce qui est problématique dans un contexte multitenant. Il n’existe pas de quota par PID ni de politique d’éviction.
Risque : un processus peut épuiser les slots et empêcher d’autres services de dériver ou charger des clés, provoquant un déni de service silencieux.
Recommandation : 
– Introduire une limite de clés par processus (ex : 4 clés) ; 
– Implémenter une politique d’expiration basée sur l’horodatage des clés et révoquer les clés inactives ; 
– Faire renvoyer un code d’erreur explicite (par exemple CRYPTO_ERR_BUSY) lorsqu’aucun slot n’est disponible.
3.4 Consolidation des opérations cryptographiques
Conformité : le crypto_server expose les primitives attendues (Blake3, XChaCha20‑Poly1305, HKDF, Ed25519, etc.) et ne retourne que des handles opaques. Les messages sont gérés via un protocole IPC avec timeouts, et un authorize_request() vérifie le token de capability avant exécution.
Améliorations : il conviendrait d’ajouter un test CI pour vérifier qu’aucun autre serveur n’importe de crates cryptographiques (RustCrypto), garantissant la règle SRV‑02.
4. Module ExoPhoenix (SSR et Handoff)
4.1 Écarts entre la spécification v6 et le code

Les spécifications v6 de ExoPhoenix_Spec (maintenant obsolètes) définissaient MAX_CORES = 64, un offset SSR PMC à 0x1080 et un format d’ACK de 64 octets. Le code et la bibliothèque exo-phoenix-ssr sont passés au layout v7 : SSR_MAX_CORES_LAYOUT = 256, un offset PMC à 0x4000 et des ACKs de 4 octets. Cette divergence est la source d’un certain nombre de tensions :

MAX_CORES et limites du kernel : bien que la lib SSR supporte 256 cœurs, plusieurs modules du noyau (isolate.rs, handoff.rs, forge.rs) conservent un garde slot >= 64. Cela empêche la prise en charge des systèmes SMP de grande taille et crée une incohérence entre la bibliothèque partagée et le code noyau.
Offsets de la SSR : les offsets de SSR_PMC, SSR_LOG_AUDIT et SSR_METRICS ont changé dans v7, mais certaines documentations (spéc v6) et modules du noyau n’ont pas été mis à jour. Cette confusion peut entraîner des accès mémoire incorrects si un développeur se réfère à la mauvaise version.
Format des ACKs de freeze : la spécification v6 prévoyait un tableau de 64 octets par cœur pour éviter le false sharing, mais la lib v7 utilise un AtomicU32 compact. Le code est cohérent avec la lib, mais la documentation doit être mise à jour pour refléter ce changement.
4.2 Reseed post‑Phoenix absent
Observation : la doc GI-05_ExoPhoenix.md exige qu’après un restore Phoenix, le noyau envoie un message PhoenixWakeEntropy au crypto_server pour réinitialiser la clé maître et le compteur de nonces. L’inspection du code n’a trouvé aucune trace de ce message ni du handler correspondant. 
Conséquence : après un snapshot/restauration, les nonces Chacha20 reprennent à la valeur du snapshot et peuvent être réutilisés, compromettant la confidentialité.
Recommandation : appliquer la correction CORR‑12. La séquence phoenix_wake_sequence() doit être introduite dans le module exophoenix/restore.rs pour générer l’entropie, l’envoyer au crypto_server, attendre un ACK et réinitialiser les structures d’IRQ, comme décrit dans les exemples de code.
4.3 Fonctions stub et limites hardcodées
KPTI stub : la fonction mark_a_pages_not_present() citée en section 1.4 reste un stub. Son implémentation est cruciale pour l’isolation mémoire lors du handoff.
FPU state freeze : lors d’un gel ExoPhoenix, l’état FPU d’un thread actif peut se trouver dans les registres physiques du CPU (lazy FPU). La correction CORR‑15 impose de forcer XSAVE avant de déclencher le snapshot. Cet aspect n’est pas encore implémenté dans le code noyau.
4.4 Mise à jour de la documentation
Spec v7 : ExoPhoenix_Spec_v7.md clarifie que cette version est la source de vérité pour le layout SSR et le contrat de handoff, et marque la version v6 comme obsolète. Elle définit explicitement SSR_MAX_CORES_LAYOUT = 256 et fournit les offsets à jour.
Recommandation : signaler clairement dans les dépôts que la spec v6 ne doit plus être utilisée, mettre à jour les commentaires dans le code noyau et aligner les modules isolate.rs, handoff.rs et forge.rs avec les constantes de la lib v7. 
Conclusion

Cette troisième passe révèle que l’architecture de sécurité d’Exo‑OS a considérablement progressé : plusieurs correctifs ont été appliqués (CET per‑thread, verify_p0_fixes, etc.), et la lib ExoPhoenix a migré vers un layout v7 plus scalable. Toutefois, des vulnérabilités critiques persistent :

Race condition au boot (CVE‑EXO‑001) – toujours non corrigée, elle permet aux APs d’exécuter des IPC avant que la sécurité ne soit prête.
Comparaisons de capacités non constant‑time – l’absence de la crate subtle expose le système aux attaques temporelles.
Fonctions stub / validations manquantes – mark_a_pages_not_present() et pmc_snapshot() nécessitent des implémentations solides.
Reseed post‑Phoenix absent – sans PhoenixWakeEntropy, la confidentialité des données chiffrées n’est pas garantie après un crash.
Nom et périmètre du serveur ExoShield – la confusion entre la couche kernel et le service Ring 1 doit être résolue.
Implémentation locale de cryptographie dans ExoShield – à supprimer au profit de crypto_server.

En appliquant les correctifs recommandés et en alignant la documentation sur les implémentations récentes, Exo‑OS peut atteindre un niveau de sécurité cohérent avec ses ambitions de plateforme renforcée. Un plan de mise à jour formel (patchs prioritaires, tests QEMU, relecture TLA+) devrait être suivi pour garantir la fermeture de ces dernières lacunes.