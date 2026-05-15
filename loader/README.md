# exo-loader

`exo-loader` est conservé comme squelette de linker dynamique pour une phase future.
En v0.2.0, le boot et `execve` utilisent le loader ELF statique du kernel
(`kernel/src/fs/elf_loader_impl.rs`) et les payloads Ring1 sont des binaires
statiques embarqués.

Le binaire bare-metal sort immédiatement avec `ENOSYS` tant que la feature
`dynamic_linking` n'est pas activée, afin d'éviter un spin silencieux si quelqu'un
essaie de le lancer comme chargeur réel.
