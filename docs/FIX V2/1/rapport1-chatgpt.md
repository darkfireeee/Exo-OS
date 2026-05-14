Incohérences restantes dans le noyau Exo‑OS après la v0.1.0 « Elder and Bobby »
Contexte

La version v0.1.0 d’Exo‑OS (nom de code « Elder and Bobby ») constitue le premier jalon où le noyau démarre, monte ExoFS et offre un shell fonctionnel. Les notes de clôture indiquent clairement que la prochaine étape, v0.2.0, vise à stabiliser complètement le noyau avant d’entamer des travaux visuels (Wayland) et l’installateur. Un audit approfondi du dépôt (commit 9bfe30a du 2026‑04‑27) croisant 728 fichiers Rust, les documents .md et les correctifs déjà appliqués a été réalisé pour recenser les incohérences et dettes techniques subsistantes.

Ce rapport recense les incohérences et problèmes majeurs mis en évidence par l’audit (version v4) et non encore résolus. Les points corrigés ne sont pas répétés ici, seuls les éléments restant à adresser pour v0.2.0 sont présentés.

Incohérences et problèmes identifiés
1. Gestion de la mémoire et Copy‑On‑Write (CoW)
Verrou global CoW – goulot d’étranglement SMP : afin de mitiger une course entre processeurs lors d’une faute de copie, le correctif BUG‑COW‑SMP‑RACE‑01 a introduit un mutex global COW_FAULT_LOCK qui sériat toutes les fautes CoW. Sur des machines multi‑cœurs, cette approche provoque une contention majeure et annule le bénéfice du multiprocesseur. Il est recommandé d’utiliser un compare_exchange sur l’entrée PTE et des verrous à granularité fine.
Tracker CoW incohérent : la fonction CowTracker::dec() retourne 0 lorsqu’elle ne trouve pas la page dans sa table, ce qui pourrait entraîner un appel erroné à free_frame(). Elle devrait renvoyer u32::MAX pour indiquer « non trouvée ».
Ref count sans verrou : CowTracker::ref_count() lit la valeur sans verrouiller la table, ouvrant une fenêtre TOCTOU sous charge SMP. Un verrou ou un accès atomique est nécessaire.
OOM killer non activé : l’infrastructure oom_kill()/register_oom_kill_sender() existe, mais le gestionnaire n’est jamais enregistré. Par défaut, nop_oom_kill() renvoie toujours false, si bien que le noyau n’élimine aucun processus et boucle indéfiniment en cas de pénurie de mémoire. Il faut appeler register_oom_kill_sender() lors de l’initialisation du scheduler.
2. Gestion des processus et ordonnancement
Reaper queue saturable : la file ring REAPER_QUEUE utilisée par le kthread reaper pour libérer les PCB et les espaces d’adressage a une taille fixe de 512 entrées. Si plus de 512 processus meurent avant que le reaper n’ait vidé la file, les entrées excédentaires sont silencieusement abandonnées et leurs ressources ne sont jamais libérées. De plus, la boucle du reaper utilise spin_loop() au lieu de dormir, consommant un CPU entier lorsque la file est vide. L’audit recommande d’augmenter la taille du ring à 4096 et d’ajouter une liste de débordement, ainsi que de remplacer le busy‑wait par un blocage réveillé par do_exit().
Appel schedule_block() en release : dans l’ordonnanceur, l’assertion de préemption utilisée pour détecter les abus de blocage disparaît en build --release. Par conséquent schedule_block() peut entrer en attente active sans déclencher d’erreur ou de mise en veille.
Offsets TCB mal documentés : la documentation GI‑01 indique que fs_base et user_gs_base se trouvent respectivement aux offsets 32 et 40 du TCB, alors que le code stocke ces champs aux offsets 72 et 80. Tout assembleur ou outil se basant sur la documentation accéderait aux mauvais champs. La documentation doit être corrigée et les offsets rendus explicites via const ou static_assert partagés.
Flag SMP_BOOT_DONE inutilisé : un indicateur global signale la fin du démarrage des processeurs secondaires, mais il n’est jamais consulté par les sous‑systèmes TLB/TSC. Cette variable est donc un code mort et pourrait masquer des incohérences d’ordre de boot.
3. Sous‑système mémoire et ordonnancement des threads
Lock CoW global trop grossier : comme indiqué ci‑dessus, la mitigation du bug SMP introduit un verrou global qui sérialise toutes les fautes CoW. Outre la contention, ce verrou empêche toute parallélisation de la duplication de pages sur des workloads intensifs.
Absence de mécanisme d’enregistrement du OOM killer : voir ci‑dessus. Sans enregistrement, la fonction par défaut nop_oom_kill() ne tue jamais de processus, ce qui mène à un kernel panic en boucle.
4. Système de fichiers ExoFS
Usage massif de .unwrap() : le module fs_bridge.rs comporte 65 appel à .unwrap() et les modules cryptographiques volume_key.rs, key_storage.rs et object_key.rs en comportent respectivement 41, 39 et 35. Ces appels risquent de provoquer un panic en espace noyau et peuvent être exploités pour déclencher un déni de service. Une propagation explicite d’erreur (Result) est nécessaire pour robustifier les chemins critiques.
Constantes on‑disk non uniformes : selon le rapport ExoFS, certaines constantes définies dans core/constants.rs ne sont pas utilisées de manière cohérente dans tous les sous‑modules. Cette divergence pourrait rendre les versions successives d’ExoFS incompatibles. Les constantes partagées doivent être centralisées dans un module commun et importées partout.
5. IPC et synchronisation
Head store Relaxed dans SPSC : dans une file SPSC, le champ head était écrit en Relaxed alors qu’il devait l’être en Release pour garantir la visibilité sur d’autres architectures. Ce point est corrigé, mais souligne l’importance de valider les ordres mémoire.
Absence de verrou pour un producteur MPSC : la file SPSC du reaper est utilisée comme queue mono-producteur/mono‑consommateur. Or le producteur peut être multiple (plusieurs appels à do_exit() sur différents cœurs). L’audit note qu’il manque un spinlock si la file venait à être utilisée en configuration MPSC.
6. Documentation vs code source
Séquence de boot incohérente : la spécification ExoOS_Architecture_v7.md décrit une séquence de 18 étapes pour le boot, alors que early_init.rs n’en comporte que 14. Les étapes manquantes (initialisation mémoire, montage ExoFS, services Ring 1) sont réparties dans lib.rs. Cette divergence peut induire des modifications incorrectes dans le bootstrap.
Commentaire sched_state obsolète : dans GI‑01, un commentaire indique que sched_state[63:32] = pid, mais dans le code le PID est stocké à l’offset 92. Ce champ a été déplacé et la documentation doit être alignée.
Ordre de verrouillage documenté mais non appliqué : la documentation indique un ordre strict des verrous (Memory → Scheduler → Security → IPC → FS) pour éviter les inversions. Aucun mécanisme de vérification n’impose cet ordre dans le code. Un LockOrder trait ou des assertions au runtime seraient nécessaires pour garantir l’absence de deadlock.
7. Divers et reliquats
Constantes inutilisées : malgré l’intégration de nombreux correctifs, des avertissements relatifs à des constantes inutilisées subsistent dans exocage.rs et exoveil.rs.
Stubs et fonctionnalités incomplètes : certaines fonctionnalités restent des stubs ou ne sont pas totalement implémentées, par exemple l’allocation de shadow stack dans exocage.rs. Ces zones doivent être complétées ou explicitement désactivées pour v0.2.0.
Augmentation du nombre de .unwrap() : depuis l’audit précédent, le nombre d’appels .unwrap() hors tests est passé de 1 982 à 2 133 (+7.6 %). Cette tendance doit être inversée car chaque unwrap() est un vecteur potentiel de plantage.
Conclusions et recommandations

L’audit démontre que de nombreux bugs critiques (P0/P1) identifiés précédemment ont été résolus, et la qualité générale du noyau progresse. Toutefois, plusieurs incohérences subsistent qui pourraient compromettre la stabilité recherchée pour v0.2.0. Les priorités identifiées sont :

Déverrouiller le chemin CoW : remplacer le verrou global par une stratégie atomique sur les PTE pour éviter la contention SMP.
Renforcer la gestion des processus : augmenter et protéger la file du reaper, corriger l’appel schedule_block() en release et enregistrer le OOM killer.
Assainir ExoFS : éliminer les .unwrap(), centraliser les constantes on‑disk et tester le GC/epoch recovery en conditions réelles.
Aligner la documentation : mettre à jour GI‑01 et Architecture v7 avec les offsets et séquences de boot réels et introduire une vérification de l’ordre des verrous.
Nettoyer les reliquats : supprimer les constantes inutilisées, compléter les stubs restants et réduire le nombre de unwrap().

En corrigeant ces incohérences et en intégrant les recommandations, la version v0.2.0 pourra servir de socle stable pour les développements visuels (Wayland, installateur) et offrir un noyau robuste, performant et conforme aux spécifications.

— Rédigé par ChatGPT, 2026‑05‑13