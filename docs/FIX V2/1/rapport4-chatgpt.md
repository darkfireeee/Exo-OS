Analyse approfondie des lenteurs des tests IPC, scheduler et FS
Contexte

Les scripts bench_ipc, bench_sched et bench_fs fournis par exosh mettent en évidence des performances très faibles : plusieurs centaines de millisecondes par appel IPC, environ 10 µs par sched_yield, et seulement ~0,03 MiB/s en écriture disque. Ces chiffres révèlent des problèmes d’implémentation plutôt qu’une limite matérielle. L’analyse suivante identifie les causes principales et propose des corrections systémiques pour que toutes les applications futures bénéficient d’une performance réaliste.

1. Sous‑système IPC
1.1 Attente bloquante basée sur un spin‑poll

L’audit « gamma » du noyau met en lumière un défaut majeur dans IpcWaitQueue : la méthode d’attente vérifie l’attribut woken par boucle de spin. Le thread occupe 100 % du cœur tant que le drapeau n’a pas été mis à true, ce qui empêche l’ordonnanceur d’exécuter d’autres tâches. La documentation explique que l’attente utilise actuellement un AtomicBool et tourne jusqu’à changement d’état . En conséquence, toute attente bloquante (rendezvous, réception synchrone, etc.) entraîne une consommation CPU inutile et dégrade drastiquement les performances en multi‑thread .

Correction proposée : remplacer le spin‑poll par une attente réellement bloquante. Le correctif CGX‑08 décrit une implémentation wait_blocking : le thread est enregistré dans SLEEP_REGISTRY, marqué comme actif, puis suspendu via un crochet de blocage fourni par l’ordonnanceur (block_current_thread). À la reprise, l’état woken est testé pour détecter un réveil ou un timeout . Cette solution permet de libérer immédiatement le CPU et d’éviter le gaspillage de cycles. Dans un noyau monocœur non initialisé, un spin de secours reste possible, mais cette voie doit être réservée aux tests unitaires .

1.2 Connexion des hooks scheduler et VMM

Le module IPC n’installe pas ses crochets (ipc_install_scheduler_hooks et ipc_install_vmm_hooks) lors de l’initialisation. La documentation insiste : si un développeur appelle ipc_init() sans avoir connecté ces hooks, l’IPC fonctionnera en mode spin‑poll silencieux pour toutes les attentes . Les hooks doivent être installés après l’initialisation du scheduler et du gestionnaire de mémoire virtuelle .

Correction proposée : modifier ipc_init() pour qu’il connecte systématiquement les hooks scheduler et VMM. Le correctif CGX‑14 montre comment appeler ipc_install_scheduler_hooks avec block_current_thread et ipc_install_vmm_hooks avec les fonctions de mappage et de démappage de pages . Sans ces appels, IpcWaitQueue::wait() ne trouve aucun crochet et se rabat sur le spin‑poll, ce qui explique les délais énormes observés dans bench_ipc.

1.3 Taille des identifiants de thread et gestion des timeouts

Les structures IpcWaiter et SleepEntry stockent l’identifiant de thread (TID) dans des AtomicU32. Or le TCB utilise des identifiants 64 bits. Dans l’audit, cette troncature est identifiée comme un problème (CGX‑06 et CGX‑07) : un TID supérieur à 2³² ne sera jamais retrouvé dans SLEEP_REGISTRY, si bien que le thread restera bloqué . Il est recommandé de promouvoir ces champs en AtomicU64 et d’adapter les lectures/écritures en conséquence .

De plus, le spin‑poll dégradé compare le nombre d’itérations à timeout_ns / 100 pour se réveiller . Avec des valeurs de timeout élevées (100 ms dans bench_ipc), cette boucle peut tourner plusieurs millions de fois avant de sortir. Une attente bloquante corrige naturellement ce problème en passant par l’ordonnanceur qui implémente un réveil par minuterie.

1.4 Autres améliorations IPC
Messages flags : l’audit du module IPC souligne la coexistence de deux types (MsgFlags et MessageFlags), des erreurs de conversion de pointeurs et des codes d’erreur dupliqués. Une unification des types et un nettoyage des unwrap()/expect() amélioreront la fiabilité et réduiront le coût du traitement des messages.
Router IPC : les tests supposent l’existence d’un service ipc_router. L’absence de ce service provoque des timeouts à 100 ms pour chaque opération. Il faut s’assurer que ipc_router est lancé au démarrage et que la résolution de noms d’endpoints est fonctionnelle.
Vérification constante : la fonction verify_cap_token doit utiliser une comparaison constant‑time pour éviter des fuites d’information. Le correctif CGX‑12 montre l’utilisation de subtle::ct_eq() pour comparer des jetons .
2. Ordonnanceur (scheduler)
2.1 Paramètres du CFS et coût du yield

Le scheduler CFS utilise des quanta de base relativement élevés : le quantum minimal est fixé à 750 µs (CFS_MIN_GRANULARITY_US) et la latence cible à 6 ms (CFS_TARGET_LATENCY_MS) . En présence d’un seul thread prêt, sched_yield réinsère le thread à la fin de la file CFS puis effectue un commutateur de contexte. Bien que la documentation annonce un coût de 500–800 cycles, les valeurs de bench_sched (~10 µs par yield) suggèrent un surcoût inattendu.

Plusieurs facteurs expliquent ce phénomène :

Instrumentation et comptage : le scheduler enregistre des compteurs (FP_YIELD_COUNT, ctx_switch_count) et des traces (idle_sched_trace). Ces mises à jour atomiques ajoutent des instructions lock et perturbent la prédiction de branche.
Synchronisation excessive : certaines opérations utilisent des ordres de mémoire trop forts ou des barrières SeqCst qui ne sont pas nécessaires sur x86_64. L’audit des structures de référence constate que les compteurs de références utilisent Relaxed à tort et recommande de passer à AcqRel pour la cohérence . Bien que cette correction vise la sûreté, elle rappelle que l’usage abusif des barres mémoire peut nuire aux performances.
Blocage absent : un bug indépendant (block_current_thread() sans vérification de préemption) permet d’appeler le blocage alors que la préemption est active. En pratique, cela peut entraîner des interversions de contextes inattendues ou des retours trop tardifs.
Calibration de l’horloge : monotonic_ns() utilise la fréquence TSC par défaut (3 GHz). Si la fréquence réelle diffère, toutes les mesures de temps sont faussées. Il est essentiel de calibrer tsc_hz au boot via le HPET ou la PIT, comme prévu par init_ktime().

Corrections proposées :

Réduction des verrous et barrières : remplacer les accès atomiques SeqCst par des ordres plus faibles (Acquire/Release) lorsque cela ne compromet pas la sûreté. L’audit note que sur x86_64, AcqRel a le même coût qu’un Relaxed grâce à l’instruction lock xadd.
Désactiver ou rendre optionnels les compteurs de debug en build release afin d’éviter la surcharge lors des appels fréquents à sched_yield.
Vérifier la préemption dans block_current_thread et ajouter une assertion (debug_assert!(preempt_count == 0)) comme recommandé par le correctif CGX‑09 pour éviter des contextes inattendus.
Calibrer la TSC lors du boot (init_ktime(tsc_now, ns_start, tsc_hz)) en mesurant la durée d’une seconde avec une source externe (HPET), afin que monotonic_ns() reflète la réalité et que les mesures de performances soient fiables.
2.2 File d’attente des threads et fausses partages

L’audit du module mémoire‑scheduler souligne plusieurs points susceptibles de réduire les performances :

Initialisation tardive du SchedNodePool : le bitmap de disponibilité est d’abord à 0, indiquant que tous les blocs sont alloués. Si une allocation survient avant l’appel d’initialisation, le noyau retourne null. L’audit recommande un test d’allocation et un panic si l’initialisation n’a pas encore été effectuée.
Alignement des structures : les structures utilisées dans les wait queues (par exemple FutexWaiter) doivent faire exactement une ligne de cache (64 octets) pour éviter le false sharing. Des assertions statiques doivent garantir cette propriété.
Nombre maximal de tâches : la constante MAX_TASKS_PER_CPU est fixée à 512 . Dans les tests, seules deux tâches sont actives, donc la structure n’est pas en cause. Toutefois, pour les futures optimisations, il serait utile de remplacer les structures statiques (tableau de 512 slots) par des listes chaînées ou des listes libres afin d’éviter les scans linéaires.
3. Système de fichiers (ExoFS)
3.1 Impact de fsync et des ouvertures/fermetures répétées

Le test bench_fs crée un fichier, écrit 1 MiB, appelle fsync, puis ferme et supprime le fichier avant de recommencer. L’API fs_fsync du noyau parcourt la table des objets, récupère le blob correspondant et persiste son contenu sur disque via persist_blob_data_if_disk . À chaque appel, les données du blob sont copiées bloc par bloc vers le périphérique virtio ; une fois terminé, un flush() est exécuté. Cette implémentation est correcte mais très coûteuse : appeler fsync après chaque mégaoctet entraîne 16 flushs pour 16 MiB et détruit le débit.

De plus, l’appel répété à openat/write/fsync/close/unlink déclenche de nombreuses allocations d’objets et insertions dans la BLOB_CACHE. Les statistiques du cache utilisent des compteurs Relaxed, ce qui peut sous‑compter certains événements et masquer les problèmes de capacité et de pression. Un balayage complet du cache est effectué pour évincer les blobs lorsque la taille maximale est atteinte, ce qui ajoute un coût quadratique lorsque de nombreux fichiers sont créés et supprimés.

Corrections proposées :

Éliminer l’appel à fsync dans le benchmark ou le remplacer par un fdatasync en fin de test. Cela reflète mieux l’usage réel (où l’on ne force pas un flush après chaque page) et améliore considérablement le débit.
Optimiser fs_fsync : plutôt que de copier l’intégralité du blob à chaque flush, maintenir un bitmask des pages modifiées et ne persister que ces pages. Une queue de travail en tâche de fond (writeback.rs) existe mais n’est pas branchée. La brancher permettrait d’effectuer les écritures en différé.
Éviter les ouvertures/fermetures répétées : réutiliser un descripteur de fichier et appeler pwrite pour écrire à différents offsets réduit les coûts du fs_bridge. Dans le noyau, l’implémentation de fs_pwrite64 est déjà disponible.
Mettre en œuvre io_uring : le module io/io_uring.rs est actuellement un stub . Son absence prive ExoFS d’un mécanisme I/O asynchrone comparable à Linux, capable de multiplier par trois le débit par rapport aux appels synchrones. Son implémentation (même limitée aux opérations disque) serait un levier majeur de performance.
3.2 Calibrage du cache et suppression des unwrap()

Le rapport souligne qu’un grand nombre d’appels unwrap()/expect() et de conversions de pointeurs non vérifiées existent dans les modules de cache, d’epoch et de storage. Ces appels engendrent des paniques ou des parcours linéaires en cas d’erreur, ce qui peut expliquer certaines lenteurs lors de tests intensifs. Il est recommandé de propager les erreurs (Result) et de documenter précisément chaque bloc unsafe plutôt que de s’en remettre à des paniques coûteuses.

Enfin, le cache de blobs (BLOB_CACHE) n’impose pas de limite stricte pour le cache de chemins (PathCache) et n’évince pas en priorité les blobs « clean » : ces incohérences peuvent mener à des évictions fréquentes et à des allocations inutiles. Une meilleure politique d’éviction (ARC/LRU) et un calibrage dynamique de la taille du cache amélioreront les performances sur de longues sessions.

4. Tests et mesures

Les correctifs ci‑dessus doivent être appliqués globalement, pas seulement au niveau des scripts de test. Une fois qu’ils sont en place :

bench_ipc devrait observer un coût constant de l’ordre de quelques microsecondes par message, sans multiplication par le nombre de threads, car l’attente sera bloquante et ne monopolise plus le CPU.
bench_sched reflétera la véritable latence d’un commutateur de contexte, typiquement < 1 µs sur une machine virtuelle lorsque les compteurs de debug sont désactivés et que la TSC est calibrée.
bench_fs dépassera plusieurs dizaines de MiB/s si l’on retire le fsync après chaque bloc, si l’écriture en différé est activée et si io_uring permet des soumissions groupées.

Pour vérifier l’impact, il est important de lancer ces benchmarks sur une machine identique avant et après corrections et de comparer les temps. En outre, l’outil de profilage perf ou un traçage dans le scheduler peut aider à localiser les points chauds restants.

Conclusion

Les lenteurs observées dans les benchmarks IPC, scheduler et FS ne proviennent pas d’une limitation inhérente au matériel mais d’incohérences dans les modules du noyau et des serveurs. Le spin‑poll dans le sous‑système IPC, l’absence de hook scheduler, les quanta de scheduling élevés, les flushs forcés et l’absence d’I/O asynchrone sont les principaux responsables. En appliquant les corrections détaillées — attente bloquante, connexion des hooks IPC, calibrage de l’horloge, optimisation du scheduler et du write‑back, implémentation d’io_uring — on peut espérer des performances comparables à celles d’un système POSIX moderne. Ces modifications améliorent l’ensemble du système : tout nouveau processus bénéficiera d’un IPC réactif, d’un ordonnanceur efficace et d’un système de fichiers rapide.