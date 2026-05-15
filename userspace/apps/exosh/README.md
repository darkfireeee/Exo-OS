# exosh prototype hôte

Ce dossier contient un prototype `std` destiné aux essais hôte Linux/musl.
Il n'est pas le shell embarqué de l'ISO ExoOS.

Le shell réellement lancé au boot est `servers/exosh/` (`exo-exosh`), compilé en
`no_std` pour la cible ExoOS et embarqué comme `/bin/exosh`.
