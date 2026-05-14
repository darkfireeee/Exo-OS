Roadmap d’adaptation des librairies réseau pour Exo‑OS

Exo‑OS utilise une architecture en micro‑noyau avec des serveurs de périphériques en ring 1 et un modèle de sécurité basé sur des capabilities. Pour fournir des fonctionnalités réseaux complètes (TCP/IP, HTTP, DNS, DHCP…), plusieurs bibliothèques ont été clonées sous libs/vendors. Ce document décrit comment adapter et intégrer ces bibliothèques dans l’écosystème Exo‑OS.

1. Bibliothèques concernées

Les clones listés dans libs.txt incluent :

Groupe	Bibliothèques	Fonctionnalités principales
Pile réseau basse – Exo‑Net	smoltcp-upstream, rtnetlink-upstream	smoltcp est une pile TCP/IP événementielle sans allocation de tas, supportant IPv4/IPv6, TCP, UDP et ICMP. rtnetlink implémente les messages netlink pour configurer les interfaces et la table de routage.
Protocoles de découverte	hickory-dns-upstream, dhcp4r-upstream	hickory-dns fournit un client/serveur DNS asynchrone complet ; dhcp4r implémente un client/serveur DHCP pour IPv4.
Transport et application HTTP	hyper-upstream, axum-upstream	hyper est une bibliothèque HTTP/1.1–2.0 cliente et serveur ; axum est un framework web basé sur tokio qui fournit un routeur et des middlewares.
TLS	rustls-upstream	Bibliothèque TLS moderne en Rust utilisée par hyper pour le HTTPS.
2. Analyse du noyau et des serveurs

Le noyau Exo‑OS expose des appels systèmes compatibles Linux (0–299) et des extensions Exo (300–399), mais il ne fournit pas de sockets TCP/IP natifs (les numéros 41–55 du tableau des syscalls sont actuellement non implémentés). La couche réseau est assurée par un serveur network_server en ring 1, qui gère le matériel (via les pilotes réseau) et fournit une API IPC pour ouvrir, envoyer et recevoir des paquets. Les clients userland envoient des messages IPC à ce serveur au lieu d’utiliser socket()/connect().

Pour intégrer des piles réseau écrites en Rust, il faut donc :

Créer une bibliothèque exo-net qui enveloppe smoltcp et expose des abstractions compatibles avec le protocole IPC existant du network_server. La bibliothèque doit instancier une interface réseau smoltcp::iface::EthernetInterface à partir de la carte réseau configurée par le serveur et fournir des sockets de haut niveau.
Adapter le network_server pour servir de backend à smoltcp :
Remplacer le code actuel de gestion des paquets par un device trait compatible smoltcp, fournissant une file d’envoi et une file de réception non bloquantes.
Ajouter un task executor minimal basé sur le scheduler d’Exo‑OS pour appeler périodiquement poll() sur la stack smoltcp et réveiller les tâches en attente. smoltcp ne nécessite pas d’allocation de tas et peut fonctionner en no_std.
Exposer un API userland similaire à socket()/bind()/listen() en traduisant ces appels en messages IPC et en les implémentant dans exo-net. Les identifiants de sockets doivent être des capabilities, distribués par le serveur réseau.
Intégrer DNS et DHCP : hickory-dns et dhcp4r devraient être compilés avec l’option no_std et utilisés côté serveur. dhcp4r configurera l’interface smoltcp (adresse IP, passerelle, DNS), tandis que hickory-dns résoudra les noms pour les applications.
HTTP et couches supérieures : pour les applications web, on peut porter hyper et axum en userland en les faisant s’exécuter sur tokio (voir document exo_runtime_adaptation.md). Ils utiliseront exo-net comme backend TCP/TLS. rustls doit être compilé avec no_std et sans dépendances à std pour fournir TLS. Des wrappers Exo‐OS devront traduire tokio::net::TcpStream en capacités réseau, et l’intégration TLS doit déléguer le chiffrement au crypto_server lorsque possible.
3. Contraintes et modifications à apporter
Désactiver std : la plupart des crates réseau doivent être compilées en mode no_std ou alloc afin de fonctionner dans un environnement sans OS. Cela implique d’utiliser l’allocation personnalisée d’Exo‑OS (exo-alloc) et d’éviter les appels POSIX.
Gestion des tâches asynchrones : smoltcp ne fournit pas de runtime ; il convient d’utiliser un exécuteur léger intégré au scheduler_server (voir exo_runtime_adaptation.md).
Sécurité capability : chaque socket ou flux doit être représenté par un capability token que le serveur réseau délivre, avec des droits précis (lecture/écriture). Les bibliothèques doivent être modifiées pour utiliser ces tokens plutôt qu’un descripteur de fichier anonyme.
Isolation : smoltcp fonctionne en espace utilisateur ; il doit être isolé dans le serveur réseau et ne pas accéder directement aux périphériques (utiliser les drivers réseau via RPC). Les piles DNS/DHCP doivent être intégrées dans ce serveur.
Interopérabilité : pour proposer des API similaires à POSIX, un thin layer exo-libc pourrait exposer socket() et rediriger vers exo-net.
4. Étapes de mise en œuvre
Prototyper exo-net : créer un crate exo-net qui dépend de smoltcp (en désactivant les features std et alloc lorsque nécessaire). Implémenter des types ExoSocket et ExoInterface reliant la stack à l’IPC du serveur réseau.
Modifier network_server : intégrer smoltcp, dhcp4r et hickory-dns et faire évoluer l’API IPC pour gérer la création et la destruction des sockets, l’envoi de segments, la réception et la gestion des timeouts. Ajouter un scheduler pour exécuter smoltcp.poll().
Adapter les applications : écrire des wrappers dans exo-libc pour exposer des fonctions compatibles POSIX (ex: connect, send, recv) qui délèguent à exo-net. Fournir des exemples de clients HTTP avec hyper/axum.
Tester la compatibilité : mesurer les performances réseau (latence, débit) et ajuster les buffers et timeouts. Exploiter les lignes d’options de smoltcp (par ex. activer log pour l’audit).
5. Conclusion

Cette feuille de route décrit comment transformer les bibliothèques réseau Rust en un sous-système exo-net cohérent, aligné avec l’architecture Exo‑OS. L’objectif est de fournir une pile réseau robuste, sécurisée et performante tout en respectant les principes de micro‑noyau et de capabilities.

Adaptation des bibliothèques cryptographiques et du serveur crypto_server :
Roadmap d’adaptation des bibliothèques cryptographiques pour Exo‑OS

Exo‑OS s’appuie sur un serveur crypto_server pour gérer les opérations cryptographiques sensibles et sur un keystore sécurisé distribuant des clés via des capabilities. Les bibliothèques clonées comprennent des implémentations Rust de primitives de chiffrement (RustCrypto, ring, libsodium, rustls) qui, une fois adaptées, offriront un ensemble complet d’algorithmes modernes aux applications.

1. Bibliothèques concernées
Groupe	Bibliothèques	Fonctionnalités principales
RustCrypto – AEAD & blocs	rustcrypto-aeads-upstream, rustcrypto-block-ciphers-upstream, rustcrypto-stream-ciphers-upstream, rustcrypto-hashes-upstream, rustcrypto-kdfs-upstream, rustcrypto-password-hashes-upstream, rustcrypto-rsa-upstream, rustcrypto-traits-upstream	Fournissent des implémentations pure‑Rust des primitives AES‑GCM, ChaCha20‑Poly1305, SHA‑2, BLAKE2, HMAC‑based KDFs, Argon2id, RSA, etc., avec une API uniforme basée sur des traits.
Librairies majeures	ring-upstream	Implémentation sécurisée et rapide de primitives cryptographiques (AEAD, ECDH, RSA, HKDF, HMAC, digest, PBKDF2), focalisée sur la sécurité et la performance, avec des routines constant‑time en assembleur.
	libsodium-upstream	Bibliothèque moderne et portable dérivée de NaCl, fournissant chiffrage, signatures, dérivation de clé et hachage, conçue pour être facile d’utilisation et extrêmement sécurisée.
	rustls-upstream	Pile TLS 1.2/1.3 en Rust utilisée par hyper; dépend de ring pour les primitives.
2. Examen du noyau et des serveurs

Le crypto_server d’Exo‑OS met à disposition des API IPC pour :

Générer, stocker et révoquer des clés symétriques ou asymétriques dans un keystore sécurisé.
Chiffrer/déchiffrer des blocs ou des flux à l’aide de ces clés.
Signer et vérifier des messages ou certificats.

L’audit sécurité a identifié plusieurs lacunes : vérification de permission non constant‑time, absence de quotas de clés par utilisateur et de prise en charge de certains algorithmes. Une refonte modulaire est envisagée pour déléguer les algorithmes à des crates Rust spécialisées.

3. Stratégie d’intégration
3.1 Regrouper les crates RustCrypto en un crate exo-crypto
Conception : créer un crate exo-crypto qui réexporte les traits des crates RustCrypto et fournit des adaptateurs vers crypto_server. Chaque algorithme sera encapsulé dans un module (ex : exo_crypto::aead::aes_gcm), exposant des fonctions encrypt(capability, nonce, aad, plaintext) -> ciphertext et decrypt(capability, nonce, aad, ciphertext) -> plaintext.
Ajustements no_std : compiler les crates RustCrypto avec default-features = false et activer la feature alloc lorsque nécessaire (ex : Argon2 utilise le tas). Les fonctions feront appel au gestionnaire de mémoire exo-alloc.
Validation de timing constant : s’assurer que les dérivations et comparaisons utilisent les fonctions constant‑time fournies par les crates (crypto_mac::MacResult, etc.) afin d’éviter des canaux auxiliaires, conformément aux recommandations de l’audit sécurité.
Interopérabilité captoken : chaque clé stockée dans le keystore Exo‑OS est identifiée par un capability. Le crate exo-crypto doit donc inclure des types SymKey/AsymKey encapsulant un ID et des droits. Les appels d’API enverront des messages IPC au crypto_server avec cet ID et les données à chiffrer/déchiffrer.
3.2 Intégrer ring et libsodium
ring : cette bibliothèque offre des implémentations éprouvées et optimisées (souvent en assembleur) de primitives AES‐GCM, ECDH, RSA, SHA‑2, HKDF et PBKDF2. On l’utilisera pour fournir des algorithmes performants lorsque les crates RustCrypto ne suffisent pas. ring sera compilé avec la feature alloc désactivée sauf pour RSA. Les appels seront encapsulés dans exo-crypto pour masquer les types internes et renvoyer des erreurs uniformes.
libsodium : cette bibliothèque C fournit un ensemble cohérent de primitives (chiffrement symétrique, signatures, hachage, dérivation de clés) et est conçue pour être facile à utiliser et portable. Pour Exo‑OS, on créera un wrapper FFI minimal en Rust, en s’assurant que les appels s’effectuent dans une enclave sécurisée (serveur crypto) pour éviter d’exposer de la mémoire sensible en userland. L’utilisation de libsodium sera limitée aux cas où aucune alternative Rust n’existe.
3.3 Gérer TLS avec rustls

rustls dépend de ring pour ses primitives et fournit une pile TLS 1.2/1.3 en Rust. Pour l’utiliser :

Compilation no_std : désactiver std et utiliser la feature alloc. L’allocation se fera via exo-alloc.
Backend réseau : adapter les ServerConnection/ClientConnection de rustls pour fonctionner au‑dessus des sockets exo-net (voir exo_net_adaptation.md). Étant donné que rustls fonctionne sur des types Read/Write, il faudra implémenter ces traits pour les capacités réseau.
Séparation privilège : conserver les opérations cryptographiques dans le crypto_server lorsque possible (ex : délégation du déchiffrement d’un record à l’API AEAD). Cela implique d’adapter rustls afin de fournir ses propres clés (via sign::CertifiedKey) à partir du keystore Exo‑OS.
4. Contraintes et modifications
Absence d’API POSIX : les bibliothèques ne doivent pas utiliser d’appels système standards (open, read, etc.). Elles doivent être compilées en no_std et utiliser des APIs Exo‑OS via des wrappers fournis par exo-libc.
Gestion du hasard : ring et libsodium nécessitent un générateur aléatoire sécurisé. Exo‑OS doit fournir un appel système ou un serveur random_server pour récupérer des entropies (par exemple via le périphérique TRNG) et le connecter aux crates via l’API getrandom().
Synchronisation : plusieurs algorithmes effectuent des opérations sur de gros buffers. Il faudra vérifier la gestion de la mémoire (pages partagées) et veiller à ne pas bloquer le scheduler_server (utiliser du preemptible).
5. Étapes de mise en œuvre
Créer le crate exo-crypto et y importer les crates RustCrypto avec les bonnes options (default-features = false, features = ["alloc"]). Définir des modules aead, digest, kdf, signature implémentant des traits uniformes et envoyant les calculs au serveur crypto.
Écrire des wrappers FFI pour ring et libsodium en Rust (extern "C" { ... }) et encapsuler les appels dans le crypto_server. S’assurer que la mémoire sensible est effacée après usage.
Étendre le crypto_server pour supporter de nouveaux algorithmes (RSA, ECDSA, Argon2id, PBKDF2, etc.), pour gérer des quotas par propriétaire et pour utiliser un PRNG matériel. La vérification des permissions doit être constant‑time.
Intégrer rustls dans exo-net pour fournir HTTPS et gRPC. Les certificats seront stockés dans le keystore et accessibles via des capabilities.
Documenter l’API pour les développeurs : décrire comment obtenir une clé, chiffrer/déchiffrer, signer/vérifier et établir une connexion TLS.
6. Conclusion

Grâce à cette feuille de route, les bibliothèques cryptographiques Rust pourront être exploitées de manière sûre et efficace dans Exo‑OS. En regroupant les primitives dans exo-crypto et en adaptant le crypto_server, on offre aux applications un ensemble de fonctionnalités comparable à celui des systèmes Unix/Windows, tout en conservant les atouts du modèle capability d’Exo‑OS.

Adaptation des systèmes de fichiers ext4/FAT/RedoxFS et intégration VFS :
Roadmap d’adaptation des bibliothèques de systèmes de fichiers pour Exo‑OS

Le noyau Exo‑OS inclut son propre système de fichiers journalisé ExoFS et un VFS minimal. Pour atteindre une compatibilité avec des formats de disque courants et offrir des fonctionnalités comparables aux systèmes Unix/Windows, plusieurs bibliothèques de systèmes de fichiers (ext2/3/4, FAT, etc.) ont été clonées. Cette feuille de route explique comment les intégrer proprement.

1. Bibliothèques concernées
Groupe	Bibliothèques	Fonctionnalités principales
ext2/3/4	ext4-rs-upstream (alias ext4_lwext4), redoxfs-upstream	ext4_lwext4 est un wrapper sûr autour de lwext4 qui permet de formater, monter, créer/éditer des fichiers et des répertoires, gérer des liens symboliques et accéder aux métadonnées. redoxfs est le système de fichiers de Redox OS, inspiré de ZFS.
FAT	rust-fatfs-upstream	Bibliothèque FAT (FAT12/16/32/exFAT) en Rust : lire/écrire des fichiers via les traits Read/Write, parcourir des répertoires, créer/supprimer des fichiers et dossiers, renommer/déplacer et gérer les métadonnées.
Fuse	libfuse-upstream (C)	Permet de monter des systèmes de fichiers en userland grâce à FUSE.
2. Contraintes du noyau et des serveurs

Le module VFS d’Exo‑OS fournit une abstraction basée sur des inodes/capabilities et repose sur ExoFS comme unique backend. Pour supporter de nouveaux formats, il faudra créer des serveurs de fichiers en ring 1, chaque serveur encapsulant une bibliothèque spécifique et se connectant au VFS via le protocole RPC existant (vfs_server). Les appels système POSIX (ex : open, read, write) seront traduits en messages IPC à ces serveurs.

3. Adaptation des bibliothèques
3.1 Créer un serveur ext_server pour ext2/3/4
Intégrer ext4_lwext4 : compiler la crate en no_std en activant la feature alloc et en remplaçant l’utilisation de std::io par des primitives de blocs fournies par Exo‑OS. Elle offre la création/montage de systèmes de fichiers, la lecture/écriture de fichiers, les opérations de répertoire et la gestion des liens.
Bloc Device : implémenter le trait BlockDevice de ext4_lwext4 en s’appuyant sur le device_server d’Exo‑OS (qui expose des capacités vers les disques). Il devra gérer la synchronisation et la mise en cache des pages.
Serveur : concevoir un ext_server qui traite les requêtes du vfs_server (open, read, write, mkdir, link, etc.) et appelle les méthodes correspondantes de la bibliothèque. Les inodes seront représentés par des capabilities.
Journalisation et quotas : s’assurer que le journal ext4 est bien flushé (via fsync) sans bloquer, et implémenter des quotas d’espace pour chaque capability (pour éviter l’épuisement de disque).
3.2 Support FAT via rust-fatfs
Compilation no_std : désactiver les features par défaut et activer alloc pour fonctionner sans std. Adapter le code pour utiliser exo-alloc comme allocateur global.
Bloc Device : créer une implémentation de fatfs::StdIoWrapper utilisant l’API device_server pour lire/écrire des secteurs. Contrôler la conversion de temps (timestamp) et les métadonnées.
Serveur fat_server : similaire à ext_server, ce serveur traduira les requêtes du VFS vers les appels de fatfs. Il gérera les caractères longs, la table FAT et la logique exFAT si nécessaire.
3.3 Support RedoxFS

redoxfs peut servir de référence pour un système de fichiers journalisé en Rust avec des snapshots. Son port nécessitera :

Compilation no_std : désactiver toutes les dépendances std et utiliser alloc.
Bloc Device : implémenter l’interface bloc sur device_server.
Serveur : un redoxfs_server peut être développé. Comme redoxfs propose déjà une API Rust avec un objet FileSystem, il suffit de créer un adaptateur vers le VFS.
3.4 FUSE pour monter d’autres fichiers

libfuse-upstream permettrait de monter des systèmes de fichiers via FUSE (par ex. NTFS, ext4). Cependant, FUSE attend une interface POSIX (read, write), incompatible avec Exo‑OS. Deux options :

Porter libfuse en Rust : écrire un wrapper exo-fuse traduisant les callbacks FUSE en messages IPC et inversement. Les opérations FUSE seraient traitées par un serveur ring 1.
Abandonner FUSE : privilégier les bibliothèques natives en Rust pour chaque format.
4. Modification du VFS et du noyau
Gestion des permissions : Exo‑OS ne possède pas de notion d’utilisateurs/groups (capabilities seules). Les bibliothèques ext2/3/4 gèrent des champs UID/GID et des bits rwx. Il faudra donc décider d’une politique : soit ignorer ces champs, soit les mapper vers des capabilities ou des labels de sécurité.
Caches et synchronisation : le VFS devra gérer plusieurs serveurs backends. Un système de cache unifié et de flushing (writeback) devra être implémenté pour éviter la corruption ou les incohérences.
API enrichie : pour se rapprocher de Linux/Windows, il faut implémenter des appels supplémentaires (statx, renameat2, copy_file_range, …). Ceux‑ci devront être routés vers le serveur approprié.
5. Étapes de mise en œuvre
Créer les serveurs spécifiques (ext_server, fat_server, redoxfs_server) et leurs BlockDevice adaptateurs.
Élargir le vfs_server pour router chaque inode vers le serveur correct selon le type de système de fichiers.
Mettre à jour musl-exo/exo-libc afin d’exposer les appels POSIX manquants (statfs, mknod, utime, etc.) et de les implémenter via l’IPC.
Écrire des outils (ex: mkfs.ext4.exo, fsck.ext4.exo) pour formater et vérifier les systèmes de fichiers en utilisant les crates Rust.
Tester la résilience : créer des tests de stress (multi‑thread, crash recovery) et des benchmarks de performance afin d’optimiser le caching et l’accès parallèle.
6. Conclusion

En ajoutant un support modulaire pour ext4, FAT et d’autres formats via des serveurs dédiés, Exo‑OS pourra monter et manipuler des volumes variés sans compromettre son architecture micro‑noyau. Ces adaptations offriront aux utilisateurs des fonctionnalités comparables à celles d’un système Linux/Windows, tout en préservant la sécurité basée sur les capabilities.

Adaptation des allocateurs mémoire (snmalloc, jemalloc, dlmalloc) :
Roadmap d’adaptation des allocateurs mémoire pour Exo‑OS

L’allocateur mémoire userland joue un rôle clé dans la stabilité et la performance des applications. Exo‑OS propose une bibliothèque exo-alloc vide qui doit être complétée et intégrée. Plusieurs allocateurs externes (dlmalloc, jemallocator, snmalloc) ont été clonés pour fournir des alternatives performantes. Cette feuille de route décrit comment les adapter au modèle d’Exo‑OS.

1. Bibliothèques concernées
Allocateur	Description	Avantages
dlmalloc-upstream	Portage Rust de Doug Lea malloc (dlmalloc). Conçu pour être simple et portable.	Faible consommation mémoire, code mature.
jemallocator-upstream	Liaison Rust vers jemalloc, allocateur haute performance utilisé par Rust avant 1.32.	Bonne scalabilité multi‑thread, faible fragmentation.
snmalloc-rs-upstream	Implémentation Rust de snmalloc, allocateur moderne optimisé pour les performances et la sécurité (pools de threads, protection contre l’utilisation de pointeurs non valides).	Très performant et sécurisé, adapté aux environnements sans OS.
2. Exigences du noyau et du runtime

Le noyau Exo‑OS fournit des appels systèmes de gestion mémoire (mmap, mprotect, mremap, munmap, brk) compatibles Linux, exposés via exo-libc. Cependant, certaines fonctionnalités manquent (support complet de madvise, mapping de grands espaces). Les allocateurs devront :

Utiliser les syscalls Exo‑OS pour réserver et libérer des pages. Par exemple, initialiser des arènes en appelant mmap avec MAP_ANONYMOUS et MAP_PRIVATE.
Gérer la concurrence via des primitives internes (verrous, listes libres) car Exo‑OS n’a pas d’API POSIX pthread_mutex en userland. L’allocateur peut utiliser des AtomicUsize et spinlocks ou s’appuyer sur les primitives du scheduler (sched_yield).
Travailler sans std : les crates Rust existantes doivent être compilées en no_std et intégrer exo-alloc comme global allocator.
3. Intégration des allocateurs
3.1 Crate exo-alloc
Sélection de l’implémentation : choisir un allocateur par défaut (snmalloc est recommandé pour ses performances et sa sécurité). L’implémentation sera encapsulée dans exo-alloc et exposée via la macro #[global_allocator] pour les programmes userland.
Adaptation : modifier exo-alloc/src/lib.rs pour importer l’allocateur choisi et l’initialiser avec des fonctions d’extension qui utilisent les syscalls mmap/munmap. L’allocation doit aligner les blocs sur la taille des pages du noyau.
Fallbacks : prévoir un second allocateur (dlmalloc) pour les plateformes où snmalloc n’est pas disponible.
3.2 jemallocator
Compilation : activer la feature use_std est interdit ; à la place, activer no_std + rusty et fournir les fonctions externes mallctl, mallctlbymib, etc. Ces fonctions devront appeler les syscalls Exo‑OS pour allouer de nouvelles arènes.
Intégration : enregistrer jemalloc comme global allocator via #[global_allocator] dans des binaires qui requièrent une faible latence et un bon comportement multi‑thread (ex : serveurs HTTP).
3.3 snmalloc-rs
Raison du choix : snmalloc est conçu pour être sûr et rapide ; il utilise une approche pool allocator avec isolement par thread et renforcement de la sécurité. Il peut être adapté à un OS sans std.
Adaptation : compiler snmalloc-rs avec la feature build_allocator et désactiver les fonctionnalités qui requièrent std. Implémenter les hooks pour la réservation et la libération de mémoire en appelant mmap et munmap via exo-libc. S’assurer que les pages réservées sont alignées sur PAGE_SIZE et respectent les protections (ex : PROT_READ|PROT_WRITE).
Integration with kernel : le noyau doit supporter les flags MAP_ANONYMOUS et MAP_PRIVATE du syscall mmap, et gérer correctement les protections pour permettre des arènes partagées.
3.4 dlmalloc

Bien qu’ancienne, cette implémentation peut servir d’alternative pour les environnements très contraints. L’adaptation consiste à l’utiliser comme fallback dans exo-alloc et à configurer ses fonctions sbrk/mmap pour qu’elles appellent les syscalls Exo‑OS.

4. Étapes de mise en œuvre
Compléter exo-alloc : importer snmalloc (ou un autre allocateur), implémenter les fonctions de réservation et libération de mémoire en utilisant mmap/munmap et déclarer l’allocateur global.
Adapter les allocateurs : compiler jemallocator, snmalloc-rs, dlmalloc en no_std, intégrer les hooks vers Exo‑OS et fournir des wrappers pour le scheduler_server afin de réduire la contention.
Tests et benchmarking : mesurer la performance (latence, fragmentation, consommation CPU) de chaque allocateur dans un programme de test. Sélectionner l’allocation par défaut en fonction des résultats.
Documentation : informer les développeurs sur le choix de l’allocateur et les moyens de le changer via un attribut #[global_allocator] propre au binaire.
5. Conclusion

En adaptant des allocateurs modernes (snmalloc, jemalloc) et en intégrant un fallback (dlmalloc), Exo‑OS offrira une gestion mémoire efficace et robuste aux programmes userland. Le crate exo-alloc deviendra une pierre angulaire du runtime, permettant de tirer parti des syscalls Exo‑OS tout en offrant des performances comparables aux systèmes traditionnels.

Adaptation des bibliothèques système (libc, PAM, udev, journalisation, services) :
Roadmap d’adaptation des bibliothèques système pour Exo‑OS

Les systèmes Unix/Windows possèdent un riche ensemble de bibliothèques système (libc, PAM, udev, journaux, etc.) fournissant des fonctionnalités standard. Exo‑OS en est encore à ses débuts : musl-exo et exo-libc implémentent une partie de la libc, mais de nombreux appels restent manquants. Ce document dresse une feuille de route pour adapter les bibliothèques système clonées afin de couvrir un spectre large de fonctionnalités tout en restant cohérent avec le modèle de capabilities.

1. Bibliothèques concernées
Bibliothèque	Usage attendu	Adaptation requise
musl-upstream, relibc-upstream, relibc-git-upstream	Libc complète (fonctions POSIX, math, stdio, réseau, process).	Portage en no_std, ajout de wrappers vers les syscalls Exo‑OS, implémentation de 200+ appels manquants (execve, fork, signals, sockets, tty).
linux-pam-upstream	Framework d’authentification modulable (Pluggable Authentication Modules).	Exo‑OS n’ayant pas de notion d’utilisateur, PAM doit être adapté pour fonctionner avec des capabilities et un keystore. Potentiellement inutilisable.
libudev-rs-upstream	Détection/gestion des périphériques (udev) en Rust.	Adapter pour interroger le device_server et recevoir des notifications hot‑plug via IPC au lieu de l’API Netlink udev.
libsodium-upstream, linux-pam	Certaines fonctions système (ex: passphrase hashing) peuvent être fournies par crypto_server.	Fournir des wrappers exo-crypto (voir exo_crypto_adaptation.md).
log-upstream, tracing-upstream	Infrastructures de journalisation et traçage.	Reconfigurer pour envoyer les logs vers le log_server/monitor_server d’Exo‑OS, utiliser un format structuré, et supporter la collecte centralisée.
pkgcraft-upstream, cargo-chef-upstream	Gestion de paquets, orchestrateurs de build.	Besoin d’un gestionnaire de paquets et d’un système de build propre à Exo‑OS (ex: exo-pkg).
launchd-upstream, systemd-upstream	Gestionnaires de services.	Étendre le init_server pour gérer des services (démarrage/arrêt, restart) à la place d’un équivalent systemd. Éventuellement s’inspirer de la structure d’unités systemd.
libudev-rs, linux-pam	Interfaces et auth.	Adaptations via capabilities.
2. Analyse du noyau et besoins

Le fichier numbers.rs du noyau définit des centaines de numéros de syscalls Linux, mais tous ne sont pas implémentés. Une étude précédente a montré qu’environ 241 appels manquent (manquants confirmés par SYSCALL_CORRECTIONS_COMPLETE_diff.md) : ceux-ci comprennent fork, execve, wait4, ptrace, socketpair, getuid, setgid, etc. Pour porter une libc complète, il faudra :

Étendre le noyau pour implémenter les appels indispensables : création de processus (fork, clone, execve), signaux (kill, sigaction), gestion de sessions (setpgid, setsid), sockets (socket, connect, sendto, etc.), gestion des utilisateurs (getuid, setuid, getgid, setgid), etc. Certains de ces appels pourront être ré-implémentés en userland (ex: fork via clone et execve), mais d’autres requièrent un support kernel.
Adapter la libc : modifier musl-exo/exo-libc pour rediriger chaque fonction vers l’API Exo (syscalls ou serveurs). Par exemple, open() deviendra un appel IPC vers vfs_server; socket() deviendra un message vers network_server (voir exo_net_adaptation.md). Des wrappers devront traduire les structures (struct stat, struct timeval, etc.) aux formats internes.
Gestion de la mémoire : implémenter mmap, mprotect, munmap, brk de manière fiable. Les pages anonymes doivent être partagées correctement avec le loader, et des protections DEP/NX doivent être respectées.
Abandonner ou remplacer PAM : le concept d’authentification par modules n’a pas de sens dans un environnement où les processus sont identifiés par des capabilities, non par des UIDs. S’il faut fournir une API pam_authenticate() pour compatibilité, celle‑ci devra vérifier la possession d’un captoken dans le keystore plutôt que de lire /etc/shadow.
Hot‑plug et udev : libudev-rs s’appuie sur Netlink et sur /sys. On peut l’adapter pour se connecter au device_server via IPC et recevoir les événements de branchement/débranchement. Les propriétés des périphériques seront exposées sous forme de capabilities.
3. Portage de musl/relibc
3.1 Approche
Créer un workspace exo-libc qui commence par un fork de musl ou relibc et remplace toutes les invocations de syscalls par des fonctions syscall_exo(n, args...) fournies par exo-libc.
Implémenter les syscalls manquants dans le noyau ou via les serveurs : par exemple, getuid() renverra une valeur virtuelle (0 par défaut), fork() créera un nouveau processus via le service process_server, wait4() utilisera la file de reaper améliorée. socket() sera traduit en message IPC.
Activer la compatibilité no_std : retirer les dépendances à glibc et aux appels pthread_*. Utiliser generic-rt pour l’accès au TLS et exo-alloc pour la mémoire.
Ajouter des fonctions spécifiques Exo‑OS : exposer des wrappers pour les syscalls 300–399 (IPC natif, capabilities, timeouts) afin d’encourager leur utilisation.
3.2 Tests
Compiler des programmes POSIX (par exemple grep, tar) contre exo-libc et exécuter sur Exo‑OS. Identifier les appels manquants et ajouter leurs implémentations.
Valider la conformité en utilisant la suite de tests de l’OpenPOSIX ou de musl pour vérifier que les fonctions standard se comportent comme attendu.
4. Intégration des autres bibliothèques
4.1 libudev-rs
Substitution de Netlink : remplacer l’utilisation de Netlink par un client IPC qui écoute les notifications du device_server. Introduire un type UdevDevice qui référence un périphérique via un captoken.
Méthodes de requête : redéfinir Device::property_value() et autres pour interroger les métadonnées exposées par le device_server.
4.2 linux-pam-upstream et shadow-rs-upstream

Ces crates gèrent les comptes utilisateurs et mots de passe (fichiers /etc/passwd et /etc/shadow). Dans Exo‑OS, il n’y a pas d’utilisateurs ; ces bibliothèques seront donc inutiles sauf si une couche d’authentification est ajoutée. Dans ce cas :

Encapsuler l’authentification : au lieu de consulter /etc/shadow, la fonction PAM devrait demander au security_server de vérifier que le processus dispose d’un captoken d’authentification.
Maintenir une base de comptes : si l’on décide d’ajouter des UID, elle sera stockée dans un serveur user_server et non dans des fichiers texte.
4.3 systemd-upstream et launchd-upstream

Ces projets sont trop lourds pour être portés entièrement. À la place :

Augmenter init_server pour gérer le démarrage et la supervision des services : lire des manifestes de service, lancer les serveurs, redémarrer en cas de crash, gérer les dépendances.
S’inspirer de systemd : adopter un format unitaire simplifié (ex : .exo-unit) décrivant le binaire, les droits (capabilities nécessaires), l’environnement et les limites de ressources.
5. Étapes de mise en œuvre
Établir la liste des appels manquants et définir lesquels doivent être implémentés dans le noyau. Créer des issues et établir des priorités.
Forker musl ou relibc et intégrer les wrappers vers Exo‑OS. Tester continuellement avec des binaires simples.
Écrire un libudev-exo : bibliothèque Rust qui expose les événements de périphériques via IPC.
Définir un init_server amélioré pour superviser les services.
Documenter l’usage : expliquer comment lier des programmes avec exo-libc et quels appels sont disponibles.
6. Conclusion

La portabilité de bibliothèques système est un travail de longue haleine. En identifiant les appels manquants et en fournissant des wrappers adaptés, Exo‑OS pourra exécuter des logiciels conçus pour Linux/Windows tout en respectant sa propre architecture (micro‑noyau, capabilities, serveurs). Certaines bibliothèques, comme PAM ou systemd, nécessiteront une réévaluation profonde, car elles reposent sur des concepts absents d’Exo‑OS.

Adaptation des runtimes et de la concurrence (Tokio, async‑std, Rayon, systemd, journaux) :
Roadmap d’adaptation des runtimes et outils de concurrence pour Exo‑OS

Les écosystèmes modernes reposent sur des runtimes asynchrones, des gestionnaires de tâches et des outils de build. Exo‑OS doit offrir des abstractions équivalentes en tirant parti de son ordonnanceur (scheduler_server) et de son modèle micro‑noyau. Ce document décrit comment intégrer tokio, async-std, rayon, ainsi que les outils cargo-chef, pkgcraft, systemd et tracing.

1. Bibliothèques concernées
Type	Bibliothèques	Fonctionnalités principales
Runtime asynchrone	tokio-upstream, async-std-upstream	Tokio fournit un scheduler multi‑thread, un système d’I/O non bloquantes, des timers et un écosystème pour le réseau. async-std offre une API proche de la bibliothèque standard avec un runtime asynchrone.
Parallélisme de données	rayon-upstream	Crate de parallélisme basé sur le work stealing, facilitant l’exécution parallèle sur plusieurs cœurs.
Outils de build & paquets	cargo-chef-upstream, pkgcraft-upstream	cargo-chef automatise la génération de caches de dépendances pour accélérer les builds. pkgcraft propose des outils pour gérer des paquets (dépôts, metadata).
Gestion de services	systemd-upstream, launchd-upstream	Démarrage et supervision des services dans Linux/macOS.
Journalisation & traçage	log-upstream, tracing-upstream	Collectent des événements et des métriques structurées. tracing fournit une instrumentation unifiée.
2. Adaptation aux mécanismes du noyau

Exo‑OS dispose d’un scheduler coopératif, exposant des appels (sys_sched_yield, sys_sched_setparam) et gérant les préemptions via scheduler_server. Les applications userland ne peuvent pas bloquer sur des appels système ordinaires : elles doivent utiliser des primitives IPC ou yield manuellement. Les runtimes asynchrones doivent donc être adaptés comme suit :

2.1 Création d’un runtime exo-rt
Construction : créer un crate exo-rt fournissant une exécution asynchrone basée sur generic-rt (accès au TLS et TCB) et sur le scheduler d’Exo‑OS. Il utilisera les appels sys_sched_yield pour céder la main lorsque les futures sont en attente.
Poule de threads : implémenter un pool work‑stealing inspiré de rayon pour répartir les tâches asynchrones sur les CPU disponibles. Les workers exécuteront des tâches jusqu’à blocage, puis appelleront sys_sched_yield pour laisser la place. Les structures de données doivent utiliser des AtomicUsize et crossbeam (si compatible no_std).
I/O non bloquante : définir des abstractions ExoAsyncRead/ExoAsyncWrite qui reposent sur sys_poll/sys_select et sur les serveurs (VFS, réseau, pipe). Le runtime devra convertir les événements poll en Waker de futures.
Timer : utiliser le time_server du noyau (ou une fonction sys_nanosleep) pour implémenter un Sleep futur. Les timers seront stockés dans une heap et réveillés via un thread superviseur.
Interopérabilité async-std/tokio : importer les crates tokio et async-std en désactivant leurs runtimes internes (default-features = false) et réimplémenter les traits Runtime/Executor pour déléguer l’exécution à exo-rt. Les primitives tokio::net et tokio::fs seront réimplémentées sur exo-net et exo-vfs.
2.2 Parallélisme de données avec rayon

rayon fonctionne en userland et utilise des primitives OS (pthread/mmap). Pour l’adapter :

Retirer les dépendances std::thread : réécrire l’implantation rayon-core pour créer des processus légers via l’API scheduler_server (sys_clone simplifié) et partager des pools d’exécution.
Synchronisation : remplacer les verrous POSIX par des structures lock‑free (ex: spin::Mutex) et des AtomicBool. Les transitions entre threads doivent passer par sys_sched_yield.
Planification : la stratégie work‑stealing reste valide ; chaque worker disposera d’une deque de tâches. La tâche de vol s’opérera en userland.
2.3 Outils de build et gestion de paquets

cargo-chef permet d’optimiser les builds en créant des images de dépendances. pkgcraft propose une interface pour manipuler des paquets (Portage/ebuild). Sur Exo‑OS, ils peuvent être utilisés tels quels dans un conteneur de build, mais :

cargo-chef : nécessite un environnement Rust complet et tar/gzip. Adapter exo-libc pour supporter les appels nécessaires (fork, execve, pipe) avant de l’utiliser. À terme, un équivalent plus simple (exo-build) pourra être écrit en utilisant le runtime exo-rt.
pkgcraft : dépend du système de fichiers et des utilisateurs. Il doit être modifié pour utiliser le VFS et un package_server (non encore existant) pour gérer l’installation, la mise à jour et la suppression des paquets sous forme de bundles de capabilities.
2.4 Gestion de services

Les clones de systemd et launchd servent d’inspiration. Dans Exo‑OS :

Étendre init_server pour lancer des services décrits dans des manifestes (chemin du binaire, arguments, capabilities requises, quotas). Le serveur devrait superviser les enfants et relancer en cas de crash.
Unités Exo : définir un format .exo décrivant les services. Les bibliothèques systemd-upstream et launchd-upstream serviront de référence, mais la plupart de leurs codes ne seront pas portés.
2.5 Journalisation et traçage

log et tracing doivent être redirigés vers un monitor_server ou un log_server. Pour ce faire :

Créer un crate exo-tracing qui implémente les traits Log et Subscriber de ces crates et envoie les messages via IPC au serveur de journalisation. tracing permet une collecte structurée et unifiée.
Instruments : fournir des macros (exo_trace!, exo_info!) qui incluent automatiquement l’ID du processus (captoken) et des métadonnées.
3. Étapes de mise en œuvre
Développer exo-rt : définir les traits d’exécution, les wakers, l’ordonnanceur et implémenter un pool de threads. Tester avec des programmes simples (async fn main).
Porter tokio/async-std : désactiver leurs runtimes, importer leurs modules utilitaires (tokio::sync, tokio::io) et réimplémenter les types TcpStream, UdpSocket, File au‑dessus des serveurs Exo.
Adapter rayon : créer un fork exo-rayon et remplacer std::thread par des appels Exo (spawn de processus légers). Fournir un API similaire (par_iter) pour les développeurs.
Mettre en place la journalisation : développer exo-tracing et un log_server capable d’agréger les traces et de les envoyer à l’outil d’analyse.
Définir les conventions : documenter comment utiliser exo-rt dans les applications, comment instrumenter le code et comment déployer des services.
4. Conclusion

L’adaptation des runtimes asynchrones et des outils de concurrence constitue une étape majeure pour offrir aux développeurs un environnement riche et performant. En créant un runtime exo-rt, en portant tokio/async-std et en intégrant rayon et les outils de build, Exo‑OS se rapprochera des capacités offertes par Linux/Windows tout en respectant son architecture unique. La journalisation centralisée permettra enfin de diagnostiquer et d’optimiser le comportement des applications.

Adaptation des bibliothèques graphiques (wgpu, winit, iced) :
Roadmap d’adaptation des bibliothèques d’interface utilisateur pour Exo‑OS

Bien qu’Exo‑OS soit orienté serveurs et embarqué, l’objectif à long terme est de proposer une interface graphique moderne. Plusieurs bibliothèques pour le rendu et la gestion des fenêtres (WGPU, Winit, Iced) ont été clonées. Ce document présente les défis et les étapes pour les intégrer.

1. Bibliothèques concernées
Bibliothèque	Fonction	Notes
wgpu-upstream	Abstraction multi‑backend pour le GPU (Vulkan/Metal/DX12/WebGPU). Fournit une API bas niveau proche de WebGPU, avec un modèle d’exécution sûr.	Utilise winit pour la gestion des surfaces, dépend de gfx-hal.
winit-upstream	Gestion multiplateforme des fenêtres, des événements clavier/souris et de la boucle principale.	Requiert un environnement windowing (X11/Wayland/Win32).
iced-upstream	Bibliothèque de GUI réactive écrite en Rust, inspirée d’Elm. Utilise wgpu et winit comme backend.	Offre des widgets, un système de messages et de commandes.
2. Contraintes d’Exo‑OS
Absence de serveur d’affichage : Exo‑OS n’intègre pas de serveur X11/Wayland. Un serveur d’affichage (display_server) devra être développé pour gérer la sortie graphique (framebuffer ou GPU) et gérer les événements d’entrée.
Accès aux périphériques : les GPU et interfaces de saisie (clavier, souris) sont pilotés par le device_server. Les bibliothèques UI devront interagir avec ce serveur via IPC.
Sécurité : les applications graphiques ne doivent pas accéder directement au framebuffer ; elles recevront des surfaces partagées via capabilities.
3. Adaptation des bibliothèques
3.1 Serveur d’affichage display_server
Architecture : créer un serveur ring 1 qui gère le GPU (via des drivers), alloue des surfaces, compose les fenêtres et envoie des événements d’entrée aux clients.
Protocole IPC : définir des messages pour créer/détruire des fenêtres, envoyer des buffers d’images, recevoir les événements et synchroniser les vblanks. Chaque fenêtre sera un captoken.
Backend GPU : utiliser wgpu côté serveur pour initialiser l’API appropriée (Vulkan ou autre). Le serveur exposera des surfaces partagées aux clients (par exemple via des shm) et recevra les commandes de dessin.
3.2 Port de winit
Loop et événements : winit gère la boucle principale et la propagation des événements. Pour Exo‑OS, il faudra adapter son backend (winit::platform) pour se connecter au display_server au lieu d’X11/Wayland. Les fonctions EventLoop::run recevront les messages d’entrée via IPC.
Surfaces : lors de la création d’une Window, winit demandera au serveur d’affichage une surface et obtiendra un captoken. Cette surface sera utilisée pour créer un wgpu::Surface.
3.3 Port de wgpu
Compilation no_std : désactiver les backends inutiles et compiler avec alloc uniquement. Adapter l’accès aux API Vulkan/Metal via les drivers du device_server (ex: passe‑plat vers l’API du système hôte si en virtualisation).
Surfaces : implémenter une couche d’intégration qui crée une wgpu::Surface à partir d’un captoken de surface envoyé par le serveur.
Synchronisation : la synchronisation GPU/CPU devra être gérée par le serveur (fences), et le client doit s’y conformer.
3.4 Port de iced
Backend personnalisé : iced repose sur wgpu et winit pour le rendu et les événements. Après adaptation de ces deux crates, il suffira d’écrire un backend iced_exo qui utilise le display_server. Les widgets et la logique réactive resteront inchangés.
Gestion des polices et ressources : charger les polices et images via vfs_server et les transférer via les surfaces de rendu.
4. Étapes de mise en œuvre
Développer display_server : mettre en place un protocole minimal (création de fenêtres, tampon d’images, gestion des entrées). Tester avec un simple programme qui dessine un rectangle.
Porter winit : créer un backend exo pour winit qui implémente EventLoop et Window via le protocole. Traduire les événements clavier/souris.
Adapter wgpu : fournir des fonctions pour créer des Surface et s’interfacer avec l’allocateur GPU du device_server. Limiter les backends à ceux supportés (par exemple Vulkan).
Intégrer iced : écrire le backend iced_exo et vérifier l’affichage de widgets simples. Ajouter un gestionnaire d’input et de focus.
Test et optimisation : mesurer la latence d’affichage, l’usage mémoire et les performances GPU. Implémenter un compositeur simple ou un gestionnaire de fenêtres pour organiser les surfaces.
5. Conclusion

L’intégration d’une pile graphique dans Exo‑OS représente un chantier à long terme. En développant un display_server et en adaptant winit, wgpu et iced, le système pourra offrir des interfaces graphiques modernes tout en conservant la sécurité et l’isolation propres au micro‑noyau. Les applications bénéficieront d’une API réactive et performante, semblable à celle des bibliothèques d’UI actuelles.

Adaptation des autres bibliothèques (axum, hyper, zbus, etc.) :
Roadmap d’adaptation des autres bibliothèques diverses pour Exo‑OS

Outre les grandes familles (réseau, crypto, système de fichiers, runtime, UI), plusieurs bibliothèques clonées fournissent des fonctionnalités complémentaires : bus de messages, frameworks Web haut niveau, gestion de configuration, etc. Ce document recense ces bibliothèques et propose des pistes d’intégration dans Exo‑OS.

1. Frameworks Web et RPC
Bibliothèque	Description	Adaptation
axum-upstream	Framework Web basé sur tokio et tower, permettant de créer des routes et des middlewares.	Utiliser exo-rt comme runtime (voir exo_runtime_adaptation.md). Adapter les types axum::Server pour écouter sur des sockets exo-net et utiliser des TLS via exo-crypto.
hyper-upstream	Bibliothèque HTTP client/serveur rapide.	Même adaptation que axum; implémenter hyper::client::connect et hyper::server::conn::Http via exo-net.
zbus-upstream	Implémentation D‑Bus en Rust (client et serveur).	Créer un équivalent exo-bus : remplacer la couche transport (sockets) par exo-net et adapter le bus pour utiliser des capabilities. Envisager un bus interne plus simple pour Exo‑OS afin de ne pas exposer d’API D‑Bus complète.

Ces frameworks requièrent un support complet des requêtes/connections et TLS (voir les roadmaps réseau et crypto).

2. Résolution de noms et configuration réseau

Les bibliothèques hickory-dns-upstream et dhcp4r-upstream sont déjà couvertes dans exo_net_adaptation.md. Elles doivent fonctionner à l’intérieur du serveur réseau pour la configuration IP et la résolution DNS. Le code userland n’utilisera donc pas directement ces crates.

3. Gestion des paquets et builds

pkgcraft-upstream et cargo-chef-upstream sont abordés dans exo_runtime_adaptation.md. Un complément :

pkgcraft : pour créer un véritable gestionnaire de paquets (exo-pkg), il faudra un serveur dédié stockant des packages en tant que capabilities (tarballs signés), une base de métadonnées et un outil CLI. pkgcraft peut servir de bibliothèque pour parser les formats existants (ebuild/portage). Le portage requiert l’ajout d’une interface no_std et la gestion des dépendances via le vfs_server.
cargo-chef : ce projet cible l’optimisation des builds Rust dans des environnements Docker. Sur Exo‑OS, il permettra de générer des couches de build pour les applications userland. Il suffit de l’exécuter dans un conteneur de développement (hors Exo‑OS) ; l’adaptation se limitera à exécuter ses binaires via exo-libc lorsqu’ils sont cross‑compilés.
4. Bibliothèques de mot de passe et shadow

shadow-rs-upstream gère la lecture de /etc/shadow et la gestion de comptes. Dans Exo‑OS, ce concept n’existe pas. Si une compatibilité est souhaitée, un user_server devra maintenir une base de comptes, et shadow-rs devra être modifié pour consulter cette base via IPC. Sinon, la bibliothèque peut être ignorée.

5. Bibliothèques utilitaires
Bibliothèque	Usage	Adaptation
jemallocator-upstream, snmalloc-rs-upstream, dlmalloc-upstream	Allocateurs mémoire	Voir exo_alloc_adaptation.md.
log-upstream, tracing-upstream	Journalisation	Voir exo_runtime_adaptation.md.
jemallocator-upstream	déjà couvert	
shadow-rs-upstream	Authentification utilisateur	Probablement inutile.
6. Conclusion

Les bibliothèques restantes apportent des fonctionnalités supplémentaires mais dépendent souvent des sous-systèmes principaux (réseau, crypto, runtime). Leur adaptation consiste essentiellement à remplacer le transport standard par des appels IPC et à utiliser les capabilities Exo‑OS pour la sécurité. En suivant les feuilles de route des autres modules, ces frameworks pourront être portés progressivement et enrichir l’écosystème d’Exo‑OS.