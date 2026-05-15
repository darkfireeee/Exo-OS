# Etat reel de validation par tests d'ExoFS

## Etat observable dans le repository

Les fichiers de tests ExoFS sont presents dans `kernel/src/fs/exofs/tests/`, dont :

- `integration/tier_1_simple.rs`
- `integration/tier_2_moyen.rs`
- `integration/tier_3_stress.rs`
- `integration/tier_4_pipeline.rs`
- `integration/tier_5_comprehensive.rs`
- `integration/tier_6_virtio_vfs.rs`
- `test_bootstrap.rs`, `test_standard.rs`, `test_stress.rs`
- les tests unitaires `unit/test_*.rs`

Le precedent rapport "0% prouve" etait donc un constat d'environnement
inaccessible, pas l'etat actuel du depot.

## Limite de preuve

Ce fichier ne remplace pas une execution CI. La validation v0.2.0 doit rester
bloquee sur l'execution effective, via WSL, des tests ExoFS critiques :

```text
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu fs::exofs::tests::integration::tier_4_pipeline
cargo test -p exo-os-kernel --target x86_64-unknown-linux-gnu fs::exofs::tests::integration::tier_6_virtio_vfs
```

Les tiers doivent expliciter leur backend :

- mock RAM accepte pour tests unitaires et tier_1/tier_2 ;
- backend VFS reel attendu pour tier_4 ;
- chemin VirtIO/VFS reel attendu pour tier_6 ;
- les tests de persistance doivent verifier remount ou reconstruction de l'etat.

## Conclusion

Couverture presente dans le code, mais preuve d'execution non inscrite ici. Un
run CI/WSL doit attacher ses logs avant de declarer ExoFS totalement valide.
