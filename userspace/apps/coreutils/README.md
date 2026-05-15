# coreutils prototype hote

Ces binaires utilisent `std` et servent aux tests hote du comportement de base
(`cat`, `echo`, `ls`, `mkdir`, `rm`, `rmdir`, `touch`).

Ils ne sont pas embarques dans l'ISO v0.2.0. Les commandes disponibles au boot
restent les built-ins de `servers/exosh/` jusqu'a ce que `fork/exec` et le
chargement des applications Ring3 externes soient valides bout-en-bout.
