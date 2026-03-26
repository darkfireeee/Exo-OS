# Audit unifie Kernel + Exo-Boot (fusion complete)

_Date: 2026-03-22_

## 1. Objectif et methode de fusion

Ce document fusionne les audits separes en un seul rapport, supprime les redondances, et recentre l analyse sur l etat reel du code kernel/src et exo-boot/src.

### 1.1 Sources fusionnees

| Source | Lignes | Usage dans la fusion |
|---|---:|---|
| AUDIT_ARCH_2026-03-22.md | 579 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_ARCH_SECONDARY_2026-03-22.md | 525 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_EXOPHOENIX_2026-03-22.md | 646 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_EXOPHOENIX_SECONDARY_2026-03-22.md | 502 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_FS_2026-03-22.md | 782 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_FS_SECONDARY_2026-03-22.md | 309 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_IPC_2026-03-22.md | 566 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_IPC_SECONDARY_2026-03-22.md | 533 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_MEMORY_2026-03-22.md | 575 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_MEMORY_SECONDARY_2026-03-22.md | 532 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_SCHEDULER_2026-03-22.md | 570 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_SCHEDULER_SECONDARY_2026-03-22.md | 552 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_SECURITY_2026-03-22.md | 561 | Consolidee et dedupliquee dans ce rapport unique |
| AUDIT_SECURITY_SECONDARY_2026-03-22.md | 546 | Consolidee et dedupliquee dans ce rapport unique |
| INDEX_AUDIT_GLOBAL_2026-03-22.md | 440 | Consolidee et dedupliquee dans ce rapport unique |

## 2. Vue ensemble chiffree etat reel

| Perimetre | Fichiers rs | Lignes approx | TODO | Stub | Placeholder |
|---|---:|---:|---:|---:|---:|
| kernel/src | 697 | 222101 | 13 | 48 | 146 |
| exo-boot/src | 30 | 5886 | 0 | 0 | 0 |
| **Total** | **727** | **227987** | **13** | **48** | **146** |

| Signaux techniques transverses | Occurrences approx | Interpretation |
|---|---:|---|
| Mutex SpinLock RwLock OnceLock | 493 | Synchronisation explicite lock ordering a verifier |
| Atomiques Atomic Ordering | 9496 | Coordination lock free et etat global |
| Vec T | 1109 | Structures dynamiques en memoire |
| Generiques T | 258 | Forte parametrisation des composants |
| unsafe | 2390 | Zones bas niveau a valider strictement |

### 2.1 Repartition par module kernel

| Module kernel | Fichiers | Lignes | TODO | Stub | Placeholder | Mutex | Atomic | Vec | Generiques | unsafe |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| fs | 292 | 130608 | 1 | 5 | 106 | 177 | 3272 | 1095 | 156 | 539 |
| memory | 128 | 27285 | 2 | 2 | 1 | 139 | 2001 | 0 | 47 | 564 |
| arch | 64 | 16026 | 2 | 24 | 10 | 1 | 854 | 0 | 1 | 431 |
| ipc | 53 | 16197 | 0 | 1 | 12 | 77 | 1452 | 0 | 17 | 312 |
| process | 44 | 7465 | 0 | 1 | 0 | 39 | 573 | 7 | 6 | 112 |
| security | 44 | 8848 | 0 | 3 | 1 | 35 | 426 | 0 | 4 | 31 |
| scheduler | 41 | 6212 | 2 | 0 | 0 | 24 | 516 | 0 | 14 | 148 |
| syscall | 21 | 6329 | 5 | 12 | 0 | 1 | 154 | 7 | 4 | 74 |
| exophoenix | 8 | 2489 | 0 | 0 | 16 | 0 | 205 | 0 | 1 | 66 |
| (racine) | 2 | 642 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 13 |

### 2.2 Repartition par module exo-boot

| Module exo-boot | Fichiers | Lignes | TODO | Stub | Placeholder | Mutex | Atomic | Vec | Generiques | unsafe |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| uefi | 10 | 1451 | 0 | 0 | 0 | 0 | 6 | 0 | 5 | 18 |
| kernel_loader | 5 | 1231 | 0 | 0 | 0 | 0 | 0 | 0 | 2 | 24 |
| memory | 4 | 1017 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 24 |
| bios | 3 | 675 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 18 |
| display | 3 | 752 | 0 | 0 | 0 | 0 | 30 | 0 | 1 | 2 |
| config | 3 | 358 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| (racine) | 2 | 402 | 0 | 0 | 0 | 0 | 7 | 0 | 0 | 14 |

## 3. Graphe ASCII fonctionnement global

~~~text
                    +-----------------------+
Firmware BIOS UEFI  |      exo-boot         |
------------------->| config memory loader  |
                    +-----------+-----------+
                                | charge kernel
                                v
                    +-----------+-----------+
                    |        kernel         |
                    | arch memory fs ipc    |
                    | scheduler security    |
                    +-----+-----+-----+-----+
                          |     |     |
                          |     |     +--> security
                          |     +--------> ipc
                          +--------------> fs

ExoPhoenix: menace exception -> sentinel -> isolate -> stage0 ssr -> handoff forge -> reprise
~~~

## 4. Arborescences exhaustives

### 4.1 kernel/src arbre complet

~~~text
├── arch
│   ├── aarch64
│   │   └── mod.rs
│   ├── mod.rs
│   ├── time.rs
│   └── x86_64
│       ├── acpi
│       │   ├── hpet.rs
│       │   ├── madt.rs
│       │   ├── mod.rs
│       │   ├── parser.rs
│       │   └── pm_timer.rs
│       ├── apic
│       │   ├── io_apic.rs
│       │   ├── ipi.rs
│       │   ├── local_apic.rs
│       │   ├── mod.rs
│       │   └── x2apic.rs
│       ├── boot
│       │   ├── early_init.rs
│       │   ├── memory_map.rs
│       │   ├── mod.rs
│       │   ├── multiboot2.rs
│       │   ├── trampoline_asm.rs
│       │   └── uefi.rs
│       ├── cpu
│       │   ├── features.rs
│       │   ├── fpu.rs
│       │   ├── mod.rs
│       │   ├── msr.rs
│       │   ├── topology.rs
│       │   └── tsc.rs
│       ├── exceptions.rs
│       ├── gdt.rs
│       ├── idt.rs
│       ├── memory_iface.rs
│       ├── mod.rs
│       ├── paging.rs
│       ├── sched_iface.rs
│       ├── smp
│       │   ├── hotplug.rs
│       │   ├── init.rs
│       │   ├── mod.rs
│       │   └── percpu.rs
│       ├── spectre
│       │   ├── ibrs.rs
│       │   ├── kpti.rs
│       │   ├── mod.rs
│       │   ├── retpoline.rs
│       │   └── ssbd.rs
│       ├── syscall.rs
│       ├── time
│       │   ├── calibration
│       │   │   ├── cpuid_nominal.rs
│       │   │   ├── mod.rs
│       │   │   ├── validation.rs
│       │   │   └── window.rs
│       │   ├── drift
│       │   │   ├── mod.rs
│       │   │   ├── periodic.rs
│       │   │   └── pll.rs
│       │   ├── ktime.rs
│       │   ├── mod.rs
│       │   ├── percpu
│       │   │   ├── mod.rs
│       │   │   └── sync.rs
│       │   └── sources
│       │       ├── hpet.rs
│       │       ├── mod.rs
│       │       ├── pit.rs
│       │       ├── pm_timer.rs
│       │       └── tsc.rs
│       ├── tss.rs
│       ├── vga_early.rs
│       └── virt
│           ├── detect.rs
│           ├── mod.rs
│           ├── paravirt.rs
│           └── stolen_time.rs
├── exophoenix
│   ├── forge.rs
│   ├── handoff.rs
│   ├── interrupts.rs
│   ├── isolate.rs
│   ├── mod.rs
│   ├── sentinel.rs
│   ├── ssr.rs
│   └── stage0.rs
├── fs
│   ├── exofs
│   │   ├── audit
│   │   │   ├── audit_entry.rs
│   │   │   ├── audit_export.rs
│   │   │   ├── audit_filter.rs
│   │   │   ├── audit_log.rs
│   │   │   ├── audit_reader.rs
│   │   │   ├── audit_rotation.rs
│   │   │   ├── audit_writer.rs
│   │   │   └── mod.rs
│   │   ├── cache
│   │   │   ├── blob_cache.rs
│   │   │   ├── cache_eviction.rs
│   │   │   ├── cache_policy.rs
│   │   │   ├── cache_pressure.rs
│   │   │   ├── cache_shrinker.rs
│   │   │   ├── cache_stats.rs
│   │   │   ├── cache_warming.rs
│   │   │   ├── extent_cache.rs
│   │   │   ├── metadata_cache.rs
│   │   │   ├── mod.rs
│   │   │   ├── object_cache.rs
│   │   │   └── path_cache.rs
│   │   ├── compress
│   │   │   ├── algorithm.rs
│   │   │   ├── compress_benchmark.rs
│   │   │   ├── compress_choice.rs
│   │   │   ├── compress_header.rs
│   │   │   ├── compress_stats.rs
│   │   │   ├── compress_threshold.rs
│   │   │   ├── compress_writer.rs
│   │   │   ├── decompress_reader.rs
│   │   │   ├── lz4_wrapper.rs
│   │   │   ├── mod.rs
│   │   │   └── zstd_wrapper.rs
│   │   ├── core
│   │   │   ├── blob_id.rs
│   │   │   ├── clock.rs
│   │   │   ├── config.rs
│   │   │   ├── constants.rs
│   │   │   ├── epoch_id.rs
│   │   │   ├── error.rs
│   │   │   ├── flags.rs
│   │   │   ├── mod.rs
│   │   │   ├── object_class.rs
│   │   │   ├── object_id.rs
│   │   │   ├── object_kind.rs
│   │   │   ├── rights.rs
│   │   │   ├── stats.rs
│   │   │   ├── types.rs
│   │   │   └── version.rs
│   │   ├── crypto
│   │   │   ├── crypto_audit.rs
│   │   │   ├── crypto_shredding.rs
│   │   │   ├── entropy.rs
│   │   │   ├── key_derivation.rs
│   │   │   ├── key_rotation.rs
│   │   │   ├── key_storage.rs
│   │   │   ├── master_key.rs
│   │   │   ├── mod.rs
│   │   │   ├── object_key.rs
│   │   │   ├── secret_reader.rs
│   │   │   ├── secret_writer.rs
│   │   │   ├── volume_key.rs
│   │   │   └── xchacha20.rs
│   │   ├── dedup
│   │   │   ├── blob_registry.rs
│   │   │   ├── blob_sharing.rs
│   │   │   ├── chunk_cache.rs
│   │   │   ├── chunk_fingerprint.rs
│   │   │   ├── chunk_index.rs
│   │   │   ├── chunker_cdc.rs
│   │   │   ├── chunker_fixed.rs
│   │   │   ├── chunking.rs
│   │   │   ├── content_hash.rs
│   │   │   ├── dedup_api.rs
│   │   │   ├── dedup_policy.rs
│   │   │   ├── dedup_stats.rs
│   │   │   ├── mod.rs
│   │   │   └── similarity_detect.rs
│   │   ├── epoch
│   │   │   ├── epoch_barriers.rs
│   │   │   ├── epoch_checksum.rs
│   │   │   ├── epoch_commit.rs
│   │   │   ├── epoch_commit_lock.rs
│   │   │   ├── epoch_delta.rs
│   │   │   ├── epoch_gc.rs
│   │   │   ├── epoch_id.rs
│   │   │   ├── epoch_pin.rs
│   │   │   ├── epoch_record.rs
│   │   │   ├── epoch_recovery.rs
│   │   │   ├── epoch_root.rs
│   │   │   ├── epoch_root_chain.rs
│   │   │   ├── epoch_slots.rs
│   │   │   ├── epoch_snapshot.rs
│   │   │   ├── epoch_stats.rs
│   │   │   ├── epoch_writeback.rs
│   │   │   └── mod.rs
│   │   ├── export
│   │   │   ├── exoar_format.rs
│   │   │   ├── exoar_reader.rs
│   │   │   ├── exoar_writer.rs
│   │   │   ├── export_audit.rs
│   │   │   ├── incremental_export.rs
│   │   │   ├── metadata_export.rs
│   │   │   ├── mod.rs
│   │   │   ├── stream_export.rs
│   │   │   ├── stream_import.rs
│   │   │   └── tar_compat.rs
│   │   ├── gc
│   │   │   ├── blob_gc.rs
│   │   │   ├── blob_refcount.rs
│   │   │   ├── cycle_detector.rs
│   │   │   ├── epoch_scanner.rs
│   │   │   ├── gc_metrics.rs
│   │   │   ├── gc_scheduler.rs
│   │   │   ├── gc_state.rs
│   │   │   ├── gc_thread.rs
│   │   │   ├── gc_tuning.rs
│   │   │   ├── inline_gc.rs
│   │   │   ├── marker.rs
│   │   │   ├── mod.rs
│   │   │   ├── orphan_collector.rs
│   │   │   ├── reference_tracker.rs
│   │   │   ├── relation_walker.rs
│   │   │   ├── sweeper.rs
│   │   │   └── tricolor.rs
│   │   ├── io
│   │   │   ├── async_io.rs
│   │   │   ├── buffered_io.rs
│   │   │   ├── direct_io.rs
│   │   │   ├── io_batch.rs
│   │   │   ├── io_stats.rs
│   │   │   ├── io_uring.rs
│   │   │   ├── mod.rs
│   │   │   ├── prefetch.rs
│   │   │   ├── readahead.rs
│   │   │   ├── reader.rs
│   │   │   ├── scatter_gather.rs
│   │   │   ├── writeback.rs
│   │   │   ├── writer.rs
│   │   │   └── zero_copy.rs
│   │   ├── lib.rs
│   │   ├── mod.rs
│   │   ├── numa
│   │   │   ├── mod.rs
│   │   │   ├── numa_affinity.rs
│   │   │   ├── numa_migration.rs
│   │   │   ├── numa_placement.rs
│   │   │   ├── numa_stats.rs
│   │   │   └── numa_tuning.rs
│   │   ├── objects
│   │   │   ├── extent.rs
│   │   │   ├── extent_tree.rs
│   │   │   ├── inline_data.rs
│   │   │   ├── logical_object.rs
│   │   │   ├── mod.rs
│   │   │   ├── object_builder.rs
│   │   │   ├── object_cache.rs
│   │   │   ├── object_kind
│   │   │   │   ├── blob.rs
│   │   │   │   ├── code.rs
│   │   │   │   ├── config.rs
│   │   │   │   ├── mod.rs
│   │   │   │   ├── path_index.rs
│   │   │   │   ├── relation.rs
│   │   │   │   └── secret.rs
│   │   │   ├── object_loader.rs
│   │   │   ├── object_meta.rs
│   │   │   ├── physical_blob.rs
│   │   │   └── physical_ref.rs
│   │   ├── observability
│   │   │   ├── alert.rs
│   │   │   ├── debug_interface.rs
│   │   │   ├── health_check.rs
│   │   │   ├── latency_histogram.rs
│   │   │   ├── metrics.rs
│   │   │   ├── mod.rs
│   │   │   ├── perf_counters.rs
│   │   │   ├── space_tracker.rs
│   │   │   ├── throughput_tracker.rs
│   │   │   └── tracing.rs
│   │   ├── path
│   │   │   ├── canonicalize.rs
│   │   │   ├── mod.rs
│   │   │   ├── mount_point.rs
│   │   │   ├── namespace.rs
│   │   │   ├── path_cache.rs
│   │   │   ├── path_component.rs
│   │   │   ├── path_index.rs
│   │   │   ├── path_index_merge.rs
│   │   │   ├── path_index_split.rs
│   │   │   ├── path_index_tree.rs
│   │   │   ├── path_walker.rs
│   │   │   ├── resolver.rs
│   │   │   └── symlink.rs
│   │   ├── posix_bridge
│   │   │   ├── fcntl_lock.rs
│   │   │   ├── inode_emulation.rs
│   │   │   ├── mmap.rs
│   │   │   ├── mod.rs
│   │   │   └── vfs_compat.rs
│   │   ├── quota
│   │   │   ├── mod.rs
│   │   │   ├── quota_audit.rs
│   │   │   ├── quota_enforcement.rs
│   │   │   ├── quota_namespace.rs
│   │   │   ├── quota_policy.rs
│   │   │   ├── quota_report.rs
│   │   │   └── quota_tracker.rs
│   │   ├── recovery
│   │   │   ├── boot_recovery.rs
│   │   │   ├── checkpoint.rs
│   │   │   ├── epoch_replay.rs
│   │   │   ├── fsck.rs
│   │   │   ├── fsck_phase1.rs
│   │   │   ├── fsck_phase2.rs
│   │   │   ├── fsck_phase3.rs
│   │   │   ├── fsck_phase4.rs
│   │   │   ├── fsck_repair.rs
│   │   │   ├── mod.rs
│   │   │   ├── recovery_audit.rs
│   │   │   ├── recovery_log.rs
│   │   │   └── slot_recovery.rs
│   │   ├── relation
│   │   │   ├── mod.rs
│   │   │   ├── relation.rs
│   │   │   ├── relation_batch.rs
│   │   │   ├── relation_cycle.rs
│   │   │   ├── relation_gc.rs
│   │   │   ├── relation_graph.rs
│   │   │   ├── relation_index.rs
│   │   │   ├── relation_query.rs
│   │   │   ├── relation_storage.rs
│   │   │   ├── relation_type.rs
│   │   │   └── relation_walker.rs
│   │   ├── snapshot
│   │   │   ├── mod.rs
│   │   │   ├── snapshot.rs
│   │   │   ├── snapshot_create.rs
│   │   │   ├── snapshot_delete.rs
│   │   │   ├── snapshot_diff.rs
│   │   │   ├── snapshot_gc.rs
│   │   │   ├── snapshot_list.rs
│   │   │   ├── snapshot_mount.rs
│   │   │   ├── snapshot_protect.rs
│   │   │   ├── snapshot_quota.rs
│   │   │   ├── snapshot_restore.rs
│   │   │   └── snapshot_streaming.rs
│   │   ├── storage
│   │   │   ├── blob_reader.rs
│   │   │   ├── blob_writer.rs
│   │   │   ├── block_allocator.rs
│   │   │   ├── block_cache.rs
│   │   │   ├── checksum_reader.rs
│   │   │   ├── checksum_writer.rs
│   │   │   ├── compression_choice.rs
│   │   │   ├── compression_reader.rs
│   │   │   ├── compression_writer.rs
│   │   │   ├── dedup_reader.rs
│   │   │   ├── dedup_writer.rs
│   │   │   ├── extent_reader.rs
│   │   │   ├── extent_writer.rs
│   │   │   ├── heap.rs
│   │   │   ├── heap_allocator.rs
│   │   │   ├── heap_coalesce.rs
│   │   │   ├── heap_free_map.rs
│   │   │   ├── io_batch.rs
│   │   │   ├── layout.rs
│   │   │   ├── mod.rs
│   │   │   ├── object_reader.rs
│   │   │   ├── object_writer.rs
│   │   │   ├── storage_stats.rs
│   │   │   ├── superblock.rs
│   │   │   ├── superblock_backup.rs
│   │   │   └── virtio_adapter.rs
│   │   ├── syscall
│   │   │   ├── epoch_commit.rs
│   │   │   ├── export_object.rs
│   │   │   ├── gc_trigger.rs
│   │   │   ├── get_content_hash.rs
│   │   │   ├── import_object.rs
│   │   │   ├── mod.rs
│   │   │   ├── object_create.rs
│   │   │   ├── object_delete.rs
│   │   │   ├── object_fd.rs
│   │   │   ├── object_open.rs
│   │   │   ├── object_read.rs
│   │   │   ├── object_set_meta.rs
│   │   │   ├── object_stat.rs
│   │   │   ├── object_write.rs
│   │   │   ├── open_by_path.rs
│   │   │   ├── path_resolve.rs
│   │   │   ├── quota_query.rs
│   │   │   ├── readdir.rs
│   │   │   ├── relation_create.rs
│   │   │   ├── relation_query.rs
│   │   │   ├── snapshot_create.rs
│   │   │   ├── snapshot_list.rs
│   │   │   ├── snapshot_mount.rs
│   │   │   └── validation.rs
│   │   └── tests
│   │       ├── integration
│   │       │   ├── mod.rs
│   │       │   ├── tier_1_simple.rs
│   │       │   ├── tier_2_moyen.rs
│   │       │   ├── tier_3_stress.rs
│   │       │   ├── tier_4_pipeline.rs
│   │       │   ├── tier_5_comprehensive.rs
│   │       │   └── tier_6_virtio_vfs.rs
│   │       ├── mod.rs
│   │       └── unit
│   │           ├── mod.rs
│   │           ├── test_blob_id.rs
│   │           ├── test_core.rs
│   │           ├── test_epoch_record.rs
│   │           └── test_xchacha20.rs
│   └── mod.rs
├── ipc
│   ├── channel
│   │   ├── async.rs
│   │   ├── broadcast.rs
│   │   ├── mod.rs
│   │   ├── mpmc.rs
│   │   ├── raw.rs
│   │   ├── streaming.rs
│   │   ├── sync.rs
│   │   └── typed.rs
│   ├── core
│   │   ├── constants.rs
│   │   ├── mod.rs
│   │   ├── sequence.rs
│   │   ├── transfer.rs
│   │   └── types.rs
│   ├── endpoint
│   │   ├── connection.rs
│   │   ├── descriptor.rs
│   │   ├── lifecycle.rs
│   │   ├── mod.rs
│   │   └── registry.rs
│   ├── message
│   │   ├── builder.rs
│   │   ├── mod.rs
│   │   ├── priority.rs
│   │   ├── router.rs
│   │   └── serializer.rs
│   ├── mod.rs
│   ├── ring
│   │   ├── batch.rs
│   │   ├── fusion.rs
│   │   ├── mod.rs
│   │   ├── mpmc.rs
│   │   ├── slot.rs
│   │   ├── spsc.rs
│   │   └── zerocopy.rs
│   ├── rpc
│   │   ├── client.rs
│   │   ├── mod.rs
│   │   ├── protocol.rs
│   │   ├── raw.rs
│   │   ├── server.rs
│   │   └── timeout.rs
│   ├── shared_memory
│   │   ├── allocator.rs
│   │   ├── descriptor.rs
│   │   ├── mapping.rs
│   │   ├── mod.rs
│   │   ├── numa_aware.rs
│   │   ├── page.rs
│   │   └── pool.rs
│   ├── stats
│   │   ├── counters.rs
│   │   └── mod.rs
│   └── sync
│       ├── barrier.rs
│       ├── event.rs
│       ├── futex.rs
│       ├── mod.rs
│       ├── rendezvous.rs
│       ├── sched_hooks.rs
│       └── wait_queue.rs
├── lib.rs
├── main.rs
├── memory
│   ├── arch_iface.rs
│   ├── core
│   │   ├── address.rs
│   │   ├── constants.rs
│   │   ├── layout.rs
│   │   ├── mod.rs
│   │   └── types.rs
│   ├── cow
│   │   ├── breaker.rs
│   │   ├── mod.rs
│   │   └── tracker.rs
│   ├── dma
│   │   ├── channels
│   │   │   ├── affinity.rs
│   │   │   ├── channel.rs
│   │   │   ├── manager.rs
│   │   │   ├── mod.rs
│   │   │   └── priority.rs
│   │   ├── completion
│   │   │   ├── handler.rs
│   │   │   ├── mod.rs
│   │   │   ├── polling.rs
│   │   │   └── wakeup.rs
│   │   ├── core
│   │   │   ├── descriptor.rs
│   │   │   ├── error.rs
│   │   │   ├── mapping.rs
│   │   │   ├── mod.rs
│   │   │   ├── types.rs
│   │   │   └── wakeup_iface.rs
│   │   ├── engines
│   │   │   ├── ahci_dma.rs
│   │   │   ├── idxd.rs
│   │   │   ├── ioat.rs
│   │   │   ├── mod.rs
│   │   │   ├── nvme_dma.rs
│   │   │   └── virtio_dma.rs
│   │   ├── iommu
│   │   │   ├── amd_iommu.rs
│   │   │   ├── arm_smmu.rs
│   │   │   ├── domain.rs
│   │   │   ├── intel_vtd.rs
│   │   │   ├── mod.rs
│   │   │   └── page_table.rs
│   │   ├── mod.rs
│   │   ├── ops
│   │   │   ├── cyclic.rs
│   │   │   ├── interleaved.rs
│   │   │   ├── memcpy.rs
│   │   │   ├── memset.rs
│   │   │   ├── mod.rs
│   │   │   └── scatter_gather.rs
│   │   └── stats
│   │       ├── counters.rs
│   │       └── mod.rs
│   ├── heap
│   │   ├── allocator
│   │   │   ├── global.rs
│   │   │   ├── hybrid.rs
│   │   │   ├── mod.rs
│   │   │   └── size_classes.rs
│   │   ├── large
│   │   │   ├── mod.rs
│   │   │   └── vmalloc.rs
│   │   ├── mod.rs
│   │   └── thread_local
│   │       ├── cache.rs
│   │       ├── drain.rs
│   │       ├── magazine.rs
│   │       └── mod.rs
│   ├── huge_pages
│   │   ├── hugetlbfs.rs
│   │   ├── mod.rs
│   │   ├── split.rs
│   │   └── thp.rs
│   ├── integrity
│   │   ├── canary.rs
│   │   ├── guard_pages.rs
│   │   ├── mod.rs
│   │   └── sanitizer.rs
│   ├── mod.rs
│   ├── numa.rs
│   ├── physical
│   │   ├── allocator
│   │   │   ├── bitmap.rs
│   │   │   ├── buddy.rs
│   │   │   ├── mod.rs
│   │   │   ├── numa_aware.rs
│   │   │   ├── numa_hints.rs
│   │   │   ├── slab.rs
│   │   │   └── slub.rs
│   │   ├── frame
│   │   │   ├── descriptor.rs
│   │   │   ├── emergency_pool.rs
│   │   │   ├── mod.rs
│   │   │   ├── pool.rs
│   │   │   ├── reclaim.rs
│   │   │   └── ref_count.rs
│   │   ├── mod.rs
│   │   ├── numa
│   │   │   ├── distance.rs
│   │   │   ├── migration.rs
│   │   │   ├── mod.rs
│   │   │   ├── node.rs
│   │   │   └── policy.rs
│   │   ├── stats.rs
│   │   └── zone
│   │       ├── dma.rs
│   │       ├── dma32.rs
│   │       ├── high.rs
│   │       ├── mod.rs
│   │       ├── movable.rs
│   │       └── normal.rs
│   ├── protection
│   │   ├── mod.rs
│   │   ├── nx.rs
│   │   ├── pku.rs
│   │   ├── smap.rs
│   │   └── smep.rs
│   ├── swap
│   │   ├── backend.rs
│   │   ├── cluster.rs
│   │   ├── compress.rs
│   │   ├── mod.rs
│   │   └── policy.rs
│   ├── utils
│   │   ├── futex_table.rs
│   │   ├── mod.rs
│   │   ├── oom_killer.rs
│   │   └── shrinker.rs
│   └── virtual
│       ├── address_space
│       │   ├── kernel.rs
│       │   ├── mapper.rs
│       │   ├── mod.rs
│       │   ├── tlb.rs
│       │   └── user.rs
│       ├── fault
│       │   ├── cow.rs
│       │   ├── demand_paging.rs
│       │   ├── handler.rs
│       │   ├── mod.rs
│       │   └── swap_in.rs
│       ├── mmap.rs
│       ├── mod.rs
│       ├── page_table
│       │   ├── builder.rs
│       │   ├── kpti_split.rs
│       │   ├── mod.rs
│       │   ├── walker.rs
│       │   └── x86_64.rs
│       └── vma
│           ├── cow.rs
│           ├── descriptor.rs
│           ├── mod.rs
│           ├── operations.rs
│           └── tree.rs
├── process
│   ├── auxv.rs
│   ├── core
│   │   ├── mod.rs
│   │   ├── pcb.rs
│   │   ├── pid.rs
│   │   ├── registry.rs
│   │   └── tcb.rs
│   ├── group
│   │   ├── job_control.rs
│   │   ├── mod.rs
│   │   ├── pgrp.rs
│   │   └── session.rs
│   ├── lifecycle
│   │   ├── create.rs
│   │   ├── exec.rs
│   │   ├── exit.rs
│   │   ├── fork.rs
│   │   ├── mod.rs
│   │   ├── reap.rs
│   │   └── wait.rs
│   ├── mod.rs
│   ├── namespace
│   │   ├── mod.rs
│   │   ├── mount_ns.rs
│   │   ├── net_ns.rs
│   │   ├── pid_ns.rs
│   │   ├── user_ns.rs
│   │   └── uts_ns.rs
│   ├── resource
│   │   ├── cgroup.rs
│   │   ├── mod.rs
│   │   ├── rlimit.rs
│   │   └── usage.rs
│   ├── signal
│   │   ├── default.rs
│   │   ├── delivery.rs
│   │   ├── handler.rs
│   │   ├── mask.rs
│   │   ├── mod.rs
│   │   ├── queue.rs
│   │   └── tcb.rs
│   ├── state
│   │   ├── mod.rs
│   │   ├── transitions.rs
│   │   └── wakeup.rs
│   └── thread
│       ├── creation.rs
│       ├── detach.rs
│       ├── join.rs
│       ├── local_storage.rs
│       ├── mod.rs
│       └── pthread_compat.rs
├── scheduler
│   ├── core
│   │   ├── mod.rs
│   │   ├── pick_next.rs
│   │   ├── preempt.rs
│   │   ├── runqueue.rs
│   │   ├── switch.rs
│   │   └── task.rs
│   ├── energy
│   │   ├── c_states.rs
│   │   ├── frequency.rs
│   │   ├── mod.rs
│   │   └── power_profile.rs
│   ├── fpu
│   │   ├── lazy.rs
│   │   ├── mod.rs
│   │   ├── save_restore.rs
│   │   └── state.rs
│   ├── mod.rs
│   ├── policies
│   │   ├── cfs.rs
│   │   ├── deadline.rs
│   │   ├── idle.rs
│   │   ├── mod.rs
│   │   └── realtime.rs
│   ├── smp
│   │   ├── affinity.rs
│   │   ├── load_balance.rs
│   │   ├── migration.rs
│   │   ├── mod.rs
│   │   └── topology.rs
│   ├── stats
│   │   ├── latency.rs
│   │   ├── mod.rs
│   │   └── per_cpu.rs
│   ├── sync
│   │   ├── barrier.rs
│   │   ├── condvar.rs
│   │   ├── mod.rs
│   │   ├── mutex.rs
│   │   ├── rwlock.rs
│   │   ├── seqlock.rs
│   │   ├── spinlock.rs
│   │   └── wait_queue.rs
│   └── timer
│       ├── clock.rs
│       ├── deadline_timer.rs
│       ├── hrtimer.rs
│       ├── mod.rs
│       └── tick.rs
├── security
│   ├── access_control
│   │   ├── checker.rs
│   │   ├── mod.rs
│   │   └── object_types.rs
│   ├── audit
│   │   ├── logger.rs
│   │   ├── mod.rs
│   │   ├── rules.rs
│   │   └── syscall_audit.rs
│   ├── capability
│   │   ├── delegation.rs
│   │   ├── mod.rs
│   │   ├── namespace.rs
│   │   ├── revocation.rs
│   │   ├── rights.rs
│   │   ├── table.rs
│   │   ├── token.rs
│   │   └── verify.rs
│   ├── crypto
│   │   ├── aes_gcm.rs
│   │   ├── blake3.rs
│   │   ├── ed25519.rs
│   │   ├── kdf.rs
│   │   ├── mod.rs
│   │   ├── rng.rs
│   │   ├── x25519.rs
│   │   └── xchacha20_poly1305.rs
│   ├── exploit_mitigations
│   │   ├── cet.rs
│   │   ├── cfg.rs
│   │   ├── kaslr.rs
│   │   ├── mod.rs
│   │   ├── safe_stack.rs
│   │   └── stack_protector.rs
│   ├── integrity_check
│   │   ├── code_signing.rs
│   │   ├── mod.rs
│   │   ├── runtime_check.rs
│   │   └── secure_boot.rs
│   ├── isolation
│   │   ├── domains.rs
│   │   ├── mod.rs
│   │   ├── namespaces.rs
│   │   ├── pledge.rs
│   │   └── sandbox.rs
│   ├── mod.rs
│   └── zero_trust
│       ├── context.rs
│       ├── labels.rs
│       ├── mod.rs
│       ├── policy.rs
│       └── verify.rs
└── syscall
    ├── abi.rs
    ├── compat
    │   ├── linux.rs
    │   ├── mod.rs
    │   └── posix.rs
    ├── dispatch.rs
    ├── entry_asm.rs
    ├── errno.rs
    ├── fast_path.rs
    ├── fs_bridge.rs
    ├── handlers
    │   ├── fd.rs
    │   ├── fs_posix.rs
    │   ├── memory.rs
    │   ├── misc.rs
    │   ├── mod.rs
    │   ├── process.rs
    │   ├── signal.rs
    │   └── time.rs
    ├── mod.rs
    ├── numbers.rs
    ├── table.rs
    └── validation.rs
~~~

### 4.2 exo-boot/src arbre complet

~~~text
├── bios
│   ├── disk.rs
│   ├── mod.rs
│   └── vga.rs
├── config
│   ├── defaults.rs
│   ├── mod.rs
│   └── parser.rs
├── display
│   ├── font.rs
│   ├── framebuffer.rs
│   └── mod.rs
├── kernel_loader
│   ├── elf.rs
│   ├── handoff.rs
│   ├── mod.rs
│   ├── relocations.rs
│   └── verify.rs
├── main.rs
├── memory
│   ├── map.rs
│   ├── mod.rs
│   ├── paging.rs
│   └── regions.rs
├── panic.rs
└── uefi
    ├── entry.rs
    ├── exit.rs
    ├── mod.rs
    ├── protocols
    │   ├── file.rs
    │   ├── graphics.rs
    │   ├── loaded_image.rs
    │   ├── mod.rs
    │   └── rng.rs
    ├── secure_boot.rs
    └── services.rs
~~~

## 5. Audit par fichier role contribution etat

Format: fichier role | L TODO STUB PH MUT ATM VEC GEN UNSAFE

### 5.1 Kernel kernel/src

#### Module fs

- kernel\src\fs\exofs\audit\audit_entry.rs - Stockage, VFS ExoFS, coherence et persistance. | L475 T0 S0 P0 M0 A2 V0 G0 U2
- kernel\src\fs\exofs\audit\audit_export.rs - Stockage, VFS ExoFS, coherence et persistance. | L425 T0 S0 P0 M0 A0 V5 G0 U0
- kernel\src\fs\exofs\audit\audit_filter.rs - Stockage, VFS ExoFS, coherence et persistance. | L443 T0 S0 P0 M0 A0 V15 G0 U0
- kernel\src\fs\exofs\audit\audit_log.rs - Stockage, VFS ExoFS, coherence et persistance. | L411 T0 S0 P0 M0 A42 V0 G1 U3
- kernel\src\fs\exofs\audit\audit_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L418 T0 S0 P0 M0 A0 V12 G2 U0
- kernel\src\fs\exofs\audit\audit_rotation.rs - Stockage, VFS ExoFS, coherence et persistance. | L430 T0 S0 P0 M0 A0 V3 G0 U0
- kernel\src\fs\exofs\audit\audit_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L415 T0 S0 P0 M0 A0 V3 G0 U0
- kernel\src\fs\exofs\audit\mod.rs - Agregateur de sous-modules et exports publics. | L64 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\cache\blob_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L378 T0 S0 P0 M3 A9 V5 G0 U0
- kernel\src\fs\exofs\cache\cache_eviction.rs - Stockage, VFS ExoFS, coherence et persistance. | L325 T0 S0 P0 M0 A0 V11 G0 U0
- kernel\src\fs\exofs\cache\cache_policy.rs - Planification, politiques execution et orchestration CPU. | L361 T0 S0 P3 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\cache\cache_pressure.rs - Stockage, VFS ExoFS, coherence et persistance. | L320 T0 S0 P0 M0 A29 V0 G0 U0
- kernel\src\fs\exofs\cache\cache_shrinker.rs - Stockage, VFS ExoFS, coherence et persistance. | L317 T0 S0 P0 M0 A2 V1 G0 U0
- kernel\src\fs\exofs\cache\cache_stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L359 T0 S0 P0 M0 A77 V0 G0 U0
- kernel\src\fs\exofs\cache\cache_warming.rs - Stockage, VFS ExoFS, coherence et persistance. | L293 T0 S0 P0 M0 A0 V5 G1 U0
- kernel\src\fs\exofs\cache\extent_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L417 T0 S0 P0 M3 A0 V7 G0 U0
- kernel\src\fs\exofs\cache\metadata_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L411 T0 S0 P0 M3 A0 V5 G1 U0
- kernel\src\fs\exofs\cache\mod.rs - Agregateur de sous-modules et exports publics. | L436 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\cache\object_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L413 T0 S0 P0 M3 A0 V2 G0 U0
- kernel\src\fs\exofs\cache\path_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L418 T0 S0 P0 M3 A16 V5 G0 U0
- kernel\src\fs\exofs\compress\algorithm.rs - Stockage, VFS ExoFS, coherence et persistance. | L447 T0 S0 P0 M0 A1 V0 G0 U0
- kernel\src\fs\exofs\compress\compress_benchmark.rs - Stockage, VFS ExoFS, coherence et persistance. | L442 T0 S0 P2 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\compress\compress_choice.rs - Stockage, VFS ExoFS, coherence et persistance. | L413 T0 S0 P31 M0 A0 V2 G0 U0
- kernel\src\fs\exofs\compress\compress_header.rs - Stockage, VFS ExoFS, coherence et persistance. | L416 T0 S0 P0 M0 A1 V0 G2 U2
- kernel\src\fs\exofs\compress\compress_stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L355 T0 S0 P0 M0 A66 V0 G0 U0
- kernel\src\fs\exofs\compress\compress_threshold.rs - Stockage, VFS ExoFS, coherence et persistance. | L343 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\compress\compress_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L410 T0 S0 P0 M0 A0 V4 G0 U0
- kernel\src\fs\exofs\compress\decompress_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L400 T0 S0 P0 M0 A0 V6 G0 U0
- kernel\src\fs\exofs\compress\lz4_wrapper.rs - Stockage, VFS ExoFS, coherence et persistance. | L86 T0 S0 P0 M0 A0 V4 G0 U0
- kernel\src\fs\exofs\compress\mod.rs - Agregateur de sous-modules et exports publics. | L405 T0 S0 P3 M0 A0 V8 G0 U0
- kernel\src\fs\exofs\compress\zstd_wrapper.rs - Stockage, VFS ExoFS, coherence et persistance. | L467 T0 S0 P0 M0 A0 V10 G0 U4
- kernel\src\fs\exofs\core\blob_id.rs - Stockage, VFS ExoFS, coherence et persistance. | L387 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\clock.rs - Stockage, VFS ExoFS, coherence et persistance. | L45 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\fs\exofs\core\config.rs - Stockage, VFS ExoFS, coherence et persistance. | L484 T0 S0 P0 M0 A78 V0 G0 U0
- kernel\src\fs\exofs\core\constants.rs - Stockage, VFS ExoFS, coherence et persistance. | L499 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\epoch_id.rs - Stockage, VFS ExoFS, coherence et persistance. | L472 T0 S0 P0 M0 A7 V0 G1 U0
- kernel\src\fs\exofs\core\error.rs - Stockage, VFS ExoFS, coherence et persistance. | L570 T0 S1 P0 M0 A0 V0 G1 U0
- kernel\src\fs\exofs\core\flags.rs - Stockage, VFS ExoFS, coherence et persistance. | L534 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\mod.rs - Agregateur de sous-modules et exports publics. | L90 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\object_class.rs - Stockage, VFS ExoFS, coherence et persistance. | L635 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\object_id.rs - Stockage, VFS ExoFS, coherence et persistance. | L541 T0 S0 P1 M0 A11 V0 G0 U1
- kernel\src\fs\exofs\core\object_kind.rs - Stockage, VFS ExoFS, coherence et persistance. | L602 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\rights.rs - Stockage, VFS ExoFS, coherence et persistance. | L522 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L542 T0 S0 P0 M0 A160 V0 G0 U0
- kernel\src\fs\exofs\core\types.rs - Stockage, VFS ExoFS, coherence et persistance. | L547 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\core\version.rs - Stockage, VFS ExoFS, coherence et persistance. | L533 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\crypto\crypto_audit.rs - Stockage, VFS ExoFS, coherence et persistance. | L433 T0 S0 P0 M0 A17 V10 G0 U4
- kernel\src\fs\exofs\crypto\crypto_shredding.rs - Stockage, VFS ExoFS, coherence et persistance. | L526 T0 S0 P0 M0 A0 V8 G3 U0
- kernel\src\fs\exofs\crypto\entropy.rs - Stockage, VFS ExoFS, coherence et persistance. | L425 T0 S0 P0 M0 A28 V1 G0 U2
- kernel\src\fs\exofs\crypto\key_derivation.rs - Stockage, VFS ExoFS, coherence et persistance. | L530 T0 S0 P0 M0 A0 V13 G0 U0
- kernel\src\fs\exofs\crypto\key_rotation.rs - Stockage, VFS ExoFS, coherence et persistance. | L432 T0 S0 P0 M0 A0 V8 G0 U0
- kernel\src\fs\exofs\crypto\key_storage.rs - Stockage, VFS ExoFS, coherence et persistance. | L494 T0 S0 P0 M0 A15 V4 G0 U15
- kernel\src\fs\exofs\crypto\master_key.rs - Stockage, VFS ExoFS, coherence et persistance. | L403 T0 S0 P0 M0 A0 V3 G0 U0
- kernel\src\fs\exofs\crypto\mod.rs - Agregateur de sous-modules et exports publics. | L550 T0 S0 P0 M0 A26 V5 G1 U2
- kernel\src\fs\exofs\crypto\object_key.rs - Stockage, VFS ExoFS, coherence et persistance. | L450 T0 S0 P0 M0 A0 V5 G0 U0
- kernel\src\fs\exofs\crypto\secret_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L605 T0 S0 P0 M0 A0 V13 G0 U0
- kernel\src\fs\exofs\crypto\secret_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L554 T0 S0 P0 M0 A6 V17 G0 U0
- kernel\src\fs\exofs\crypto\volume_key.rs - Stockage, VFS ExoFS, coherence et persistance. | L435 T0 S0 P0 M0 A0 V5 G0 U0
- kernel\src\fs\exofs\crypto\xchacha20.rs - Stockage, VFS ExoFS, coherence et persistance. | L472 T0 S0 P0 M0 A6 V5 G0 U0
- kernel\src\fs\exofs\dedup\blob_registry.rs - Stockage, VFS ExoFS, coherence et persistance. | L454 T0 S0 P0 M0 A22 V11 G0 U3
- kernel\src\fs\exofs\dedup\blob_sharing.rs - Communication inter-processus et transport de messages. | L442 T0 S0 P0 M0 A16 V11 G0 U3
- kernel\src\fs\exofs\dedup\chunk_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L437 T0 S0 P0 M0 A28 V5 G0 U3
- kernel\src\fs\exofs\dedup\chunk_fingerprint.rs - Stockage, VFS ExoFS, coherence et persistance. | L421 T0 S0 P0 M0 A0 V3 G0 U0
- kernel\src\fs\exofs\dedup\chunk_index.rs - Stockage, VFS ExoFS, coherence et persistance. | L429 T0 S0 P0 M0 A22 V4 G0 U3
- kernel\src\fs\exofs\dedup\chunker_cdc.rs - Stockage, VFS ExoFS, coherence et persistance. | L464 T0 S0 P19 M0 A0 V5 G0 U0
- kernel\src\fs\exofs\dedup\chunker_fixed.rs - Stockage, VFS ExoFS, coherence et persistance. | L505 T0 S0 P0 M0 A0 V16 G0 U0
- kernel\src\fs\exofs\dedup\chunking.rs - Stockage, VFS ExoFS, coherence et persistance. | L432 T0 S0 P0 M0 A0 V9 G0 U0
- kernel\src\fs\exofs\dedup\content_hash.rs - Stockage, VFS ExoFS, coherence et persistance. | L460 T0 S0 P1 M0 A28 V4 G0 U7
- kernel\src\fs\exofs\dedup\dedup_api.rs - Stockage, VFS ExoFS, coherence et persistance. | L431 T0 S0 P0 M0 A0 V3 G0 U0
- kernel\src\fs\exofs\dedup\dedup_policy.rs - Planification, politiques execution et orchestration CPU. | L421 T0 S0 P0 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\dedup\dedup_stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L457 T0 S0 P0 M0 A64 V2 G0 U0
- kernel\src\fs\exofs\dedup\mod.rs - Agregateur de sous-modules et exports publics. | L408 T0 S0 P1 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\dedup\similarity_detect.rs - Stockage, VFS ExoFS, coherence et persistance. | L420 T0 S0 P0 M0 A0 V20 G0 U0
- kernel\src\fs\exofs\epoch\epoch_barriers.rs - Stockage, VFS ExoFS, coherence et persistance. | L402 T0 S2 P0 M0 A46 V0 G0 U2
- kernel\src\fs\exofs\epoch\epoch_checksum.rs - Stockage, VFS ExoFS, coherence et persistance. | L367 T0 S0 P0 M0 A0 V0 G0 U2
- kernel\src\fs\exofs\epoch\epoch_commit.rs - Stockage, VFS ExoFS, coherence et persistance. | L344 T0 S0 P0 M0 A0 V0 G2 U1
- kernel\src\fs\exofs\epoch\epoch_commit_lock.rs - Stockage, VFS ExoFS, coherence et persistance. | L294 T0 S0 P0 M6 A14 V0 G1 U0
- kernel\src\fs\exofs\epoch\epoch_delta.rs - Stockage, VFS ExoFS, coherence et persistance. | L493 T0 S0 P0 M0 A0 V1 G2 U0
- kernel\src\fs\exofs\epoch\epoch_gc.rs - Stockage, VFS ExoFS, coherence et persistance. | L523 T0 S0 P0 M1 A20 V4 G0 U0
- kernel\src\fs\exofs\epoch\epoch_id.rs - Stockage, VFS ExoFS, coherence et persistance. | L495 T0 S0 P1 M0 A36 V0 G0 U0
- kernel\src\fs\exofs\epoch\epoch_pin.rs - Stockage, VFS ExoFS, coherence et persistance. | L424 T0 S0 P0 M4 A6 V1 G0 U0
- kernel\src\fs\exofs\epoch\epoch_record.rs - Stockage, VFS ExoFS, coherence et persistance. | L517 T1 S0 P0 M0 A1 V0 G1 U22
- kernel\src\fs\exofs\epoch\epoch_recovery.rs - Stockage, VFS ExoFS, coherence et persistance. | L581 T0 S0 P0 M0 A0 V3 G1 U0
- kernel\src\fs\exofs\epoch\epoch_root.rs - Stockage, VFS ExoFS, coherence et persistance. | L486 T0 S0 P0 M0 A0 V4 G0 U2
- kernel\src\fs\exofs\epoch\epoch_root_chain.rs - Stockage, VFS ExoFS, coherence et persistance. | L387 T0 S0 P1 M0 A0 V10 G0 U5
- kernel\src\fs\exofs\epoch\epoch_slots.rs - Stockage, VFS ExoFS, coherence et persistance. | L414 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\epoch\epoch_snapshot.rs - Stockage, VFS ExoFS, coherence et persistance. | L513 T0 S0 P0 M5 A0 V3 G1 U0
- kernel\src\fs\exofs\epoch\epoch_stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L493 T0 S0 P0 M0 A114 V0 G0 U0
- kernel\src\fs\exofs\epoch\epoch_writeback.rs - Stockage, VFS ExoFS, coherence et persistance. | L689 T0 S0 P0 M1 A55 V0 G0 U0
- kernel\src\fs\exofs\epoch\mod.rs - Agregateur de sous-modules et exports publics. | L318 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\export\exoar_format.rs - Stockage, VFS ExoFS, coherence et persistance. | L591 T0 S0 P0 M0 A0 V0 G0 U21
- kernel\src\fs\exofs\export\exoar_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L772 T0 S0 P0 M0 A0 V8 G4 U16
- kernel\src\fs\exofs\export\exoar_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L635 T0 S0 P0 M0 A0 V8 G6 U0
- kernel\src\fs\exofs\export\export_audit.rs - Stockage, VFS ExoFS, coherence et persistance. | L743 T0 S0 P0 M0 A22 V0 G0 U7
- kernel\src\fs\exofs\export\incremental_export.rs - Stockage, VFS ExoFS, coherence et persistance. | L663 T0 S0 P0 M0 A0 V7 G2 U0
- kernel\src\fs\exofs\export\metadata_export.rs - Stockage, VFS ExoFS, coherence et persistance. | L708 T0 S0 P0 M0 A0 V7 G6 U3
- kernel\src\fs\exofs\export\mod.rs - Agregateur de sous-modules et exports publics. | L645 T0 S0 P0 M0 A0 V11 G3 U0
- kernel\src\fs\exofs\export\stream_export.rs - Stockage, VFS ExoFS, coherence et persistance. | L757 T0 S0 P0 M0 A0 V6 G2 U0
- kernel\src\fs\exofs\export\stream_import.rs - Stockage, VFS ExoFS, coherence et persistance. | L769 T0 S0 P0 M0 A0 V5 G2 U6
- kernel\src\fs\exofs\export\tar_compat.rs - Stockage, VFS ExoFS, coherence et persistance. | L877 T0 S0 P0 M0 A0 V15 G7 U4
- kernel\src\fs\exofs\gc\blob_gc.rs - Stockage, VFS ExoFS, coherence et persistance. | L526 T0 S0 P0 M3 A0 V0 G2 U0
- kernel\src\fs\exofs\gc\blob_refcount.rs - Stockage, VFS ExoFS, coherence et persistance. | L566 T0 S0 P0 M5 A19 V3 G0 U0
- kernel\src\fs\exofs\gc\cycle_detector.rs - Stockage, VFS ExoFS, coherence et persistance. | L517 T0 S0 P0 M3 A0 V8 G0 U0
- kernel\src\fs\exofs\gc\epoch_scanner.rs - Stockage, VFS ExoFS, coherence et persistance. | L563 T0 S0 P0 M3 A0 V2 G3 U0
- kernel\src\fs\exofs\gc\gc_metrics.rs - Stockage, VFS ExoFS, coherence et persistance. | L407 T0 S0 P0 M0 A86 V0 G0 U0
- kernel\src\fs\exofs\gc\gc_scheduler.rs - Planification, politiques execution et orchestration CPU. | L412 T0 S0 P0 M3 A15 V0 G0 U0
- kernel\src\fs\exofs\gc\gc_state.rs - Stockage, VFS ExoFS, coherence et persistance. | L539 T0 S0 P0 M6 A19 V0 G0 U0
- kernel\src\fs\exofs\gc\gc_thread.rs - Stockage, VFS ExoFS, coherence et persistance. | L409 T0 S0 P0 M0 A42 V0 G0 U0
- kernel\src\fs\exofs\gc\gc_tuning.rs - Stockage, VFS ExoFS, coherence et persistance. | L451 T0 S0 P0 M3 A16 V0 G0 U0
- kernel\src\fs\exofs\gc\inline_gc.rs - Stockage, VFS ExoFS, coherence et persistance. | L473 T0 S0 P0 M3 A0 V1 G0 U0
- kernel\src\fs\exofs\gc\marker.rs - Stockage, VFS ExoFS, coherence et persistance. | L459 T0 S0 P0 M3 A0 V1 G0 U0
- kernel\src\fs\exofs\gc\mod.rs - Agregateur de sous-modules et exports publics. | L222 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\gc\orphan_collector.rs - Stockage, VFS ExoFS, coherence et persistance. | L400 T0 S0 P0 M3 A0 V3 G0 U0
- kernel\src\fs\exofs\gc\reference_tracker.rs - Stockage, VFS ExoFS, coherence et persistance. | L400 T0 S0 P0 M4 A0 V11 G0 U0
- kernel\src\fs\exofs\gc\relation_walker.rs - Stockage, VFS ExoFS, coherence et persistance. | L417 T0 S0 P0 M3 A0 V6 G0 U0
- kernel\src\fs\exofs\gc\sweeper.rs - Stockage, VFS ExoFS, coherence et persistance. | L432 T0 S0 P0 M3 A0 V3 G0 U0
- kernel\src\fs\exofs\gc\tricolor.rs - Stockage, VFS ExoFS, coherence et persistance. | L557 T0 S0 P0 M0 A0 V2 G0 U0
- kernel\src\fs\exofs\io\async_io.rs - Stockage, VFS ExoFS, coherence et persistance. | L543 T0 S0 P0 M0 A78 V0 G0 U10
- kernel\src\fs\exofs\io\buffered_io.rs - Stockage, VFS ExoFS, coherence et persistance. | L439 T0 S0 P0 M0 A0 V3 G3 U0
- kernel\src\fs\exofs\io\direct_io.rs - Stockage, VFS ExoFS, coherence et persistance. | L434 T0 S0 P0 M0 A0 V2 G0 U0
- kernel\src\fs\exofs\io\io_batch.rs - Stockage, VFS ExoFS, coherence et persistance. | L412 T0 S0 P0 M0 A0 V8 G1 U0
- kernel\src\fs\exofs\io\io_stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L643 T0 S0 P0 M0 A135 V0 G0 U7
- kernel\src\fs\exofs\io\io_uring.rs - Communication inter-processus et transport de messages. | L470 T0 S0 P0 M0 A47 V3 G0 U9
- kernel\src\fs\exofs\io\mod.rs - Agregateur de sous-modules et exports publics. | L417 T0 S0 P0 M0 A21 V1 G2 U2
- kernel\src\fs\exofs\io\prefetch.rs - Stockage, VFS ExoFS, coherence et persistance. | L408 T0 S0 P10 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\io\readahead.rs - Stockage, VFS ExoFS, coherence et persistance. | L455 T0 S0 P18 M0 A0 V3 G0 U0
- kernel\src\fs\exofs\io\reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L504 T0 S0 P0 M0 A0 V4 G4 U0
- kernel\src\fs\exofs\io\scatter_gather.rs - Stockage, VFS ExoFS, coherence et persistance. | L437 T0 S0 P0 M0 A0 V4 G0 U5
- kernel\src\fs\exofs\io\writeback.rs - Stockage, VFS ExoFS, coherence et persistance. | L495 T0 S0 P0 M0 A25 V3 G0 U6
- kernel\src\fs\exofs\io\writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L522 T0 S0 P0 M0 A0 V6 G4 U0
- kernel\src\fs\exofs\io\zero_copy.rs - Stockage, VFS ExoFS, coherence et persistance. | L452 T0 S0 P0 M0 A0 V5 G5 U0
- kernel\src\fs\exofs\lib.rs - Surface API globale et orchestration des modules. | L16 T0 S0 P0 M0 A1 V0 G0 U0
- kernel\src\fs\exofs\mod.rs - Agregateur de sous-modules et exports publics. | L144 T0 S0 P1 M1 A7 V0 G0 U0
- kernel\src\fs\exofs\numa\mod.rs - Agregateur de sous-modules et exports publics. | L443 T0 S0 P1 M0 A5 V1 G0 U7
- kernel\src\fs\exofs\numa\numa_affinity.rs - Stockage, VFS ExoFS, coherence et persistance. | L454 T0 S0 P0 M1 A6 V2 G0 U15
- kernel\src\fs\exofs\numa\numa_migration.rs - Stockage, VFS ExoFS, coherence et persistance. | L532 T0 S0 P0 M0 A36 V1 G0 U9
- kernel\src\fs\exofs\numa\numa_placement.rs - Stockage, VFS ExoFS, coherence et persistance. | L401 T0 S0 P0 M0 A34 V1 G0 U0
- kernel\src\fs\exofs\numa\numa_stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L434 T0 S0 P0 M0 A61 V1 G0 U0
- kernel\src\fs\exofs\numa\numa_tuning.rs - Stockage, VFS ExoFS, coherence et persistance. | L444 T0 S0 P2 M0 A54 V0 G0 U0
- kernel\src\fs\exofs\objects\extent.rs - Stockage, VFS ExoFS, coherence et persistance. | L606 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\objects\extent_tree.rs - Stockage, VFS ExoFS, coherence et persistance. | L651 T0 S0 P0 M0 A0 V5 G1 U0
- kernel\src\fs\exofs\objects\inline_data.rs - Stockage, VFS ExoFS, coherence et persistance. | L514 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\fs\exofs\objects\logical_object.rs - Stockage, VFS ExoFS, coherence et persistance. | L591 T0 S0 P0 M3 A19 V0 G0 U1
- kernel\src\fs\exofs\objects\mod.rs - Agregateur de sous-modules et exports publics. | L228 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\objects\object_builder.rs - Stockage, VFS ExoFS, coherence et persistance. | L489 T0 S0 P0 M3 A8 V0 G1 U0
- kernel\src\fs\exofs\objects\object_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L469 T0 S0 P0 M5 A0 V2 G1 U0
- kernel\src\fs\exofs\objects\object_kind\blob.rs - Stockage, VFS ExoFS, coherence et persistance. | L552 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\fs\exofs\objects\object_kind\code.rs - Stockage, VFS ExoFS, coherence et persistance. | L534 T0 S0 P0 M0 A0 V1 G0 U1
- kernel\src\fs\exofs\objects\object_kind\config.rs - Stockage, VFS ExoFS, coherence et persistance. | L557 T0 S0 P0 M0 A0 V4 G0 U1
- kernel\src\fs\exofs\objects\object_kind\mod.rs - Agregateur de sous-modules et exports publics. | L117 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\objects\object_kind\path_index.rs - Stockage, VFS ExoFS, coherence et persistance. | L569 T0 S0 P0 M0 A3 V3 G0 U1
- kernel\src\fs\exofs\objects\object_kind\relation.rs - Stockage, VFS ExoFS, coherence et persistance. | L614 T0 S0 P0 M0 A0 V7 G0 U1
- kernel\src\fs\exofs\objects\object_kind\secret.rs - Stockage, VFS ExoFS, coherence et persistance. | L486 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\fs\exofs\objects\object_loader.rs - Stockage, VFS ExoFS, coherence et persistance. | L309 T0 S0 P0 M3 A0 V1 G0 U4
- kernel\src\fs\exofs\objects\object_meta.rs - Stockage, VFS ExoFS, coherence et persistance. | L722 T0 S0 P0 M0 A1 V0 G2 U1
- kernel\src\fs\exofs\objects\physical_blob.rs - Stockage, VFS ExoFS, coherence et persistance. | L614 T0 S0 P0 M1 A12 V1 G0 U1
- kernel\src\fs\exofs\objects\physical_ref.rs - Stockage, VFS ExoFS, coherence et persistance. | L379 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\observability\alert.rs - Stockage, VFS ExoFS, coherence et persistance. | L418 T0 S0 P0 M0 A36 V4 G1 U6
- kernel\src\fs\exofs\observability\debug_interface.rs - Stockage, VFS ExoFS, coherence et persistance. | L531 T0 S0 P0 M0 A41 V2 G1 U6
- kernel\src\fs\exofs\observability\health_check.rs - Stockage, VFS ExoFS, coherence et persistance. | L458 T0 S0 P0 M0 A15 V1 G0 U9
- kernel\src\fs\exofs\observability\latency_histogram.rs - Stockage, VFS ExoFS, coherence et persistance. | L433 T0 S0 P0 M0 A50 V1 G0 U3
- kernel\src\fs\exofs\observability\metrics.rs - Stockage, VFS ExoFS, coherence et persistance. | L408 T0 S0 P0 M0 A14 V1 G0 U4
- kernel\src\fs\exofs\observability\mod.rs - Agregateur de sous-modules et exports publics. | L445 T0 S0 P0 M0 A9 V1 G0 U0
- kernel\src\fs\exofs\observability\perf_counters.rs - Stockage, VFS ExoFS, coherence et persistance. | L437 T0 S0 P0 M0 A24 V1 G0 U3
- kernel\src\fs\exofs\observability\space_tracker.rs - Stockage, VFS ExoFS, coherence et persistance. | L486 T0 S0 P0 M0 A60 V1 G0 U3
- kernel\src\fs\exofs\observability\throughput_tracker.rs - Stockage, VFS ExoFS, coherence et persistance. | L500 T0 S0 P0 M0 A71 V2 G0 U10
- kernel\src\fs\exofs\observability\tracing.rs - Stockage, VFS ExoFS, coherence et persistance. | L494 T0 S0 P0 M0 A31 V3 G1 U7
- kernel\src\fs\exofs\path\canonicalize.rs - Stockage, VFS ExoFS, coherence et persistance. | L429 T0 S0 P0 M0 A0 V13 G0 U0
- kernel\src\fs\exofs\path\mod.rs - Agregateur de sous-modules et exports publics. | L255 T0 S0 P0 M0 A1 V0 G0 U0
- kernel\src\fs\exofs\path\mount_point.rs - Stockage, VFS ExoFS, coherence et persistance. | L401 T0 S0 P0 M4 A1 V0 G1 U0
- kernel\src\fs\exofs\path\namespace.rs - Stockage, VFS ExoFS, coherence et persistance. | L405 T0 S0 P0 M4 A1 V2 G1 U0
- kernel\src\fs\exofs\path\path_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L403 T0 S0 P0 M4 A0 V4 G0 U0
- kernel\src\fs\exofs\path\path_component.rs - Stockage, VFS ExoFS, coherence et persistance. | L526 T0 S0 P0 M0 A0 V5 G1 U3
- kernel\src\fs\exofs\path\path_index.rs - Stockage, VFS ExoFS, coherence et persistance. | L646 T0 S0 P0 M0 A23 V8 G0 U2
- kernel\src\fs\exofs\path\path_index_merge.rs - Stockage, VFS ExoFS, coherence et persistance. | L452 T0 S0 P0 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\path\path_index_split.rs - Stockage, VFS ExoFS, coherence et persistance. | L429 T0 S0 P0 M0 A0 V4 G0 U0
- kernel\src\fs\exofs\path\path_index_tree.rs - Stockage, VFS ExoFS, coherence et persistance. | L435 T0 S0 P0 M0 A0 V2 G2 U0
- kernel\src\fs\exofs\path\path_walker.rs - Stockage, VFS ExoFS, coherence et persistance. | L409 T0 S0 P0 M0 A0 V7 G4 U0
- kernel\src\fs\exofs\path\resolver.rs - Stockage, VFS ExoFS, coherence et persistance. | L406 T0 S0 P0 M0 A0 V8 G5 U0
- kernel\src\fs\exofs\path\symlink.rs - Stockage, VFS ExoFS, coherence et persistance. | L474 T0 S0 P0 M3 A0 V8 G1 U0
- kernel\src\fs\exofs\posix_bridge\fcntl_lock.rs - Stockage, VFS ExoFS, coherence et persistance. | L466 T0 S0 P0 M0 A13 V3 G0 U10
- kernel\src\fs\exofs\posix_bridge\inode_emulation.rs - Stockage, VFS ExoFS, coherence et persistance. | L482 T0 S0 P0 M0 A13 V4 G0 U18
- kernel\src\fs\exofs\posix_bridge\mmap.rs - Stockage, VFS ExoFS, coherence et persistance. | L569 T0 S0 P0 M0 A21 V3 G0 U14
- kernel\src\fs\exofs\posix_bridge\mod.rs - Agregateur de sous-modules et exports publics. | L606 T0 S0 P0 M0 A36 V0 G1 U0
- kernel\src\fs\exofs\posix_bridge\vfs_compat.rs - Stockage, VFS ExoFS, coherence et persistance. | L595 T0 S0 P0 M0 A15 V3 G0 U8
- kernel\src\fs\exofs\quota\mod.rs - Agregateur de sous-modules et exports publics. | L451 T0 S0 P0 M0 A5 V1 G0 U7
- kernel\src\fs\exofs\quota\quota_audit.rs - Stockage, VFS ExoFS, coherence et persistance. | L568 T0 S0 P0 M0 A44 V3 G1 U5
- kernel\src\fs\exofs\quota\quota_enforcement.rs - Stockage, VFS ExoFS, coherence et persistance. | L514 T0 S0 P0 M0 A5 V1 G0 U5
- kernel\src\fs\exofs\quota\quota_namespace.rs - Stockage, VFS ExoFS, coherence et persistance. | L599 T0 S0 P0 M0 A16 V2 G0 U17
- kernel\src\fs\exofs\quota\quota_policy.rs - Planification, politiques execution et orchestration CPU. | L510 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\quota\quota_report.rs - Stockage, VFS ExoFS, coherence et persistance. | L454 T0 S0 P0 M0 A0 V7 G0 U0
- kernel\src\fs\exofs\quota\quota_tracker.rs - Stockage, VFS ExoFS, coherence et persistance. | L634 T0 S0 P0 M0 A43 V2 G0 U17
- kernel\src\fs\exofs\recovery\boot_recovery.rs - Briques bas niveau architecture CPU boot et interruptions. | L428 T0 S0 P0 M0 A6 V0 G0 U0
- kernel\src\fs\exofs\recovery\checkpoint.rs - Stockage, VFS ExoFS, coherence et persistance. | L612 T0 S0 P0 M3 A6 V1 G0 U3
- kernel\src\fs\exofs\recovery\epoch_replay.rs - Stockage, VFS ExoFS, coherence et persistance. | L567 T0 S0 P0 M0 A5 V0 G0 U4
- kernel\src\fs\exofs\recovery\fsck.rs - Stockage, VFS ExoFS, coherence et persistance. | L562 T0 S0 P0 M0 A7 V0 G0 U0
- kernel\src\fs\exofs\recovery\fsck_phase1.rs - Stockage, VFS ExoFS, coherence et persistance. | L514 T0 S0 P0 M0 A1 V3 G0 U4
- kernel\src\fs\exofs\recovery\fsck_phase2.rs - Stockage, VFS ExoFS, coherence et persistance. | L514 T0 S0 P0 M0 A1 V3 G0 U3
- kernel\src\fs\exofs\recovery\fsck_phase3.rs - Stockage, VFS ExoFS, coherence et persistance. | L761 T0 S0 P0 M0 A1 V3 G0 U6
- kernel\src\fs\exofs\recovery\fsck_phase4.rs - Stockage, VFS ExoFS, coherence et persistance. | L594 T0 S1 P0 M0 A0 V4 G0 U4
- kernel\src\fs\exofs\recovery\fsck_repair.rs - Stockage, VFS ExoFS, coherence et persistance. | L633 T0 S0 P0 M0 A16 V2 G0 U9
- kernel\src\fs\exofs\recovery\mod.rs - Agregateur de sous-modules et exports publics. | L293 T0 S0 P0 M0 A13 V0 G0 U0
- kernel\src\fs\exofs\recovery\recovery_audit.rs - Stockage, VFS ExoFS, coherence et persistance. | L708 T0 S0 P0 M0 A31 V5 G0 U5
- kernel\src\fs\exofs\recovery\recovery_log.rs - Stockage, VFS ExoFS, coherence et persistance. | L587 T0 S0 P0 M0 A26 V4 G0 U5
- kernel\src\fs\exofs\recovery\slot_recovery.rs - Stockage, VFS ExoFS, coherence et persistance. | L552 T0 S0 P0 M0 A5 V1 G0 U6
- kernel\src\fs\exofs\relation\mod.rs - Agregateur de sous-modules et exports publics. | L62 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\relation\relation.rs - Stockage, VFS ExoFS, coherence et persistance. | L401 T0 S0 P0 M0 A2 V0 G0 U2
- kernel\src\fs\exofs\relation\relation_batch.rs - Stockage, VFS ExoFS, coherence et persistance. | L442 T0 S0 P0 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\relation\relation_cycle.rs - Stockage, VFS ExoFS, coherence et persistance. | L423 T0 S0 P0 M0 A0 V11 G0 U0
- kernel\src\fs\exofs\relation\relation_gc.rs - Stockage, VFS ExoFS, coherence et persistance. | L438 T0 S0 P0 M0 A0 V2 G0 U0
- kernel\src\fs\exofs\relation\relation_graph.rs - Stockage, VFS ExoFS, coherence et persistance. | L399 T0 S0 P0 M3 A11 V11 G0 U0
- kernel\src\fs\exofs\relation\relation_index.rs - Stockage, VFS ExoFS, coherence et persistance. | L407 T0 S0 P0 M3 A0 V14 G0 U0
- kernel\src\fs\exofs\relation\relation_query.rs - Stockage, VFS ExoFS, coherence et persistance. | L400 T0 S0 P0 M0 A0 V15 G1 U0
- kernel\src\fs\exofs\relation\relation_storage.rs - Stockage, VFS ExoFS, coherence et persistance. | L407 T0 S0 P0 M5 A7 V10 G0 U0
- kernel\src\fs\exofs\relation\relation_type.rs - Stockage, VFS ExoFS, coherence et persistance. | L424 T0 S0 P0 M0 A1 V0 G0 U0
- kernel\src\fs\exofs\relation\relation_walker.rs - Stockage, VFS ExoFS, coherence et persistance. | L576 T0 S0 P0 M0 A0 V9 G0 U0
- kernel\src\fs\exofs\snapshot\mod.rs - Agregateur de sous-modules et exports publics. | L191 T0 S0 P0 M0 A1 V0 G0 U0
- kernel\src\fs\exofs\snapshot\snapshot.rs - Stockage, VFS ExoFS, coherence et persistance. | L443 T0 S0 P0 M0 A2 V1 G1 U3
- kernel\src\fs\exofs\snapshot\snapshot_create.rs - Stockage, VFS ExoFS, coherence et persistance. | L397 T0 S0 P0 M0 A0 V5 G0 U0
- kernel\src\fs\exofs\snapshot\snapshot_delete.rs - Stockage, VFS ExoFS, coherence et persistance. | L333 T0 S0 P0 M0 A0 V8 G1 U0
- kernel\src\fs\exofs\snapshot\snapshot_diff.rs - Stockage, VFS ExoFS, coherence et persistance. | L433 T0 S0 P0 M0 A6 V10 G2 U0
- kernel\src\fs\exofs\snapshot\snapshot_gc.rs - Stockage, VFS ExoFS, coherence et persistance. | L377 T0 S0 P0 M0 A0 V4 G0 U0
- kernel\src\fs\exofs\snapshot\snapshot_list.rs - Stockage, VFS ExoFS, coherence et persistance. | L355 T0 S0 P0 M3 A28 V8 G1 U0
- kernel\src\fs\exofs\snapshot\snapshot_mount.rs - Stockage, VFS ExoFS, coherence et persistance. | L348 T0 S0 P0 M3 A12 V4 G0 U0
- kernel\src\fs\exofs\snapshot\snapshot_protect.rs - Stockage, VFS ExoFS, coherence et persistance. | L348 T0 S0 P0 M3 A14 V4 G0 U0
- kernel\src\fs\exofs\snapshot\snapshot_quota.rs - Stockage, VFS ExoFS, coherence et persistance. | L364 T0 S0 P0 M5 A22 V4 G0 U0
- kernel\src\fs\exofs\snapshot\snapshot_restore.rs - Stockage, VFS ExoFS, coherence et persistance. | L377 T0 S0 P0 M0 A7 V8 G3 U0
- kernel\src\fs\exofs\snapshot\snapshot_streaming.rs - Stockage, VFS ExoFS, coherence et persistance. | L459 T0 S0 P0 M0 A12 V8 G6 U2
- kernel\src\fs\exofs\storage\blob_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L802 T0 S0 P0 M0 A44 V21 G7 U1
- kernel\src\fs\exofs\storage\blob_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L864 T0 S0 P0 M0 A31 V6 G4 U4
- kernel\src\fs\exofs\storage\block_allocator.rs - Gestion memoire physique virtuelle DMA et protection. | L401 T0 S0 P0 M5 A17 V2 G0 U0
- kernel\src\fs\exofs\storage\block_cache.rs - Stockage, VFS ExoFS, coherence et persistance. | L477 T0 S0 P0 M5 A23 V8 G0 U0
- kernel\src\fs\exofs\storage\checksum_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L468 T0 S0 P0 M0 A0 V11 G1 U0
- kernel\src\fs\exofs\storage\checksum_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L486 T0 S0 P0 M0 A0 V10 G0 U0
- kernel\src\fs\exofs\storage\compression_choice.rs - Stockage, VFS ExoFS, coherence et persistance. | L463 T0 S0 P1 M0 A21 V3 G0 U0
- kernel\src\fs\exofs\storage\compression_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L483 T0 S0 P0 M0 A10 V15 G0 U0
- kernel\src\fs\exofs\storage\compression_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L344 T0 S1 P1 M0 A0 V11 G0 U0
- kernel\src\fs\exofs\storage\dedup_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L423 T0 S0 P0 M4 A14 V9 G1 U0
- kernel\src\fs\exofs\storage\dedup_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L458 T0 S0 P0 M3 A13 V4 G0 U0
- kernel\src\fs\exofs\storage\extent_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L416 T0 S0 P0 M0 A19 V12 G0 U0
- kernel\src\fs\exofs\storage\extent_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L485 T0 S0 P0 M0 A19 V8 G0 U0
- kernel\src\fs\exofs\storage\heap.rs - Gestion memoire physique virtuelle DMA et protection. | L412 T0 S0 P0 M1 A11 V0 G0 U0
- kernel\src\fs\exofs\storage\heap_allocator.rs - Gestion memoire physique virtuelle DMA et protection. | L472 T0 S0 P0 M6 A23 V0 G0 U0
- kernel\src\fs\exofs\storage\heap_coalesce.rs - Gestion memoire physique virtuelle DMA et protection. | L428 T0 S0 P0 M1 A0 V2 G0 U0
- kernel\src\fs\exofs\storage\heap_free_map.rs - Gestion memoire physique virtuelle DMA et protection. | L469 T0 S0 P0 M1 A0 V5 G0 U0
- kernel\src\fs\exofs\storage\io_batch.rs - Stockage, VFS ExoFS, coherence et persistance. | L554 T0 S0 P0 M0 A11 V10 G1 U0
- kernel\src\fs\exofs\storage\layout.rs - Stockage, VFS ExoFS, coherence et persistance. | L561 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\storage\mod.rs - Agregateur de sous-modules et exports publics. | L503 T0 S0 P1 M0 A12 V2 G1 U0
- kernel\src\fs\exofs\storage\object_reader.rs - Stockage, VFS ExoFS, coherence et persistance. | L836 T0 S0 P0 M0 A42 V24 G8 U1
- kernel\src\fs\exofs\storage\object_writer.rs - Stockage, VFS ExoFS, coherence et persistance. | L797 T0 S0 P0 M0 A27 V5 G5 U3
- kernel\src\fs\exofs\storage\storage_stats.rs - Stockage, VFS ExoFS, coherence et persistance. | L601 T0 S0 P0 M1 A130 V0 G0 U0
- kernel\src\fs\exofs\storage\superblock.rs - Stockage, VFS ExoFS, coherence et persistance. | L811 T0 S0 P0 M6 A50 V4 G6 U3
- kernel\src\fs\exofs\storage\superblock_backup.rs - Stockage, VFS ExoFS, coherence et persistance. | L466 T0 S0 P0 M0 A0 V1 G0 U5
- kernel\src\fs\exofs\storage\virtio_adapter.rs - Stockage, VFS ExoFS, coherence et persistance. | L49 T0 S0 P5 M5 A0 V0 G0 U0
- kernel\src\fs\exofs\syscall\epoch_commit.rs - Stockage, VFS ExoFS, coherence et persistance. | L476 T0 S0 P0 M0 A18 V6 G0 U5
- kernel\src\fs\exofs\syscall\export_object.rs - Stockage, VFS ExoFS, coherence et persistance. | L471 T0 S0 P0 M0 A0 V18 G0 U2
- kernel\src\fs\exofs\syscall\gc_trigger.rs - Stockage, VFS ExoFS, coherence et persistance. | L451 T0 S0 P0 M0 A7 V6 G0 U2
- kernel\src\fs\exofs\syscall\get_content_hash.rs - Stockage, VFS ExoFS, coherence et persistance. | L531 T0 S0 P0 M0 A0 V7 G0 U2
- kernel\src\fs\exofs\syscall\import_object.rs - Stockage, VFS ExoFS, coherence et persistance. | L447 T0 S0 P0 M0 A0 V8 G0 U4
- kernel\src\fs\exofs\syscall\mod.rs - Agregateur de sous-modules et exports publics. | L439 T0 S0 P0 M0 A24 V0 G1 U0
- kernel\src\fs\exofs\syscall\object_create.rs - Stockage, VFS ExoFS, coherence et persistance. | L502 T0 S0 P0 M0 A0 V5 G0 U2
- kernel\src\fs\exofs\syscall\object_delete.rs - Stockage, VFS ExoFS, coherence et persistance. | L483 T0 S0 P0 M0 A0 V5 G0 U2
- kernel\src\fs\exofs\syscall\object_fd.rs - Stockage, VFS ExoFS, coherence et persistance. | L618 T0 S0 P0 M2 A30 V0 G0 U12
- kernel\src\fs\exofs\syscall\object_open.rs - Stockage, VFS ExoFS, coherence et persistance. | L460 T0 S0 P0 M0 A0 V2 G0 U1
- kernel\src\fs\exofs\syscall\object_read.rs - Stockage, VFS ExoFS, coherence et persistance. | L485 T0 S0 P0 M0 A0 V3 G0 U2
- kernel\src\fs\exofs\syscall\object_set_meta.rs - Stockage, VFS ExoFS, coherence et persistance. | L482 T0 S0 P0 M0 A0 V16 G0 U3
- kernel\src\fs\exofs\syscall\object_stat.rs - Stockage, VFS ExoFS, coherence et persistance. | L542 T0 S0 P0 M0 A0 V5 G0 U3
- kernel\src\fs\exofs\syscall\object_write.rs - Stockage, VFS ExoFS, coherence et persistance. | L503 T0 S0 P2 M0 A0 V4 G0 U2
- kernel\src\fs\exofs\syscall\open_by_path.rs - Stockage, VFS ExoFS, coherence et persistance. | L131 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\syscall\path_resolve.rs - Stockage, VFS ExoFS, coherence et persistance. | L483 T0 S0 P0 M0 A0 V2 G1 U1
- kernel\src\fs\exofs\syscall\quota_query.rs - Stockage, VFS ExoFS, coherence et persistance. | L434 T0 S0 P0 M0 A0 V3 G0 U5
- kernel\src\fs\exofs\syscall\readdir.rs - Stockage, VFS ExoFS, coherence et persistance. | L235 T0 S0 P0 M0 A0 V4 G0 U1
- kernel\src\fs\exofs\syscall\relation_create.rs - Stockage, VFS ExoFS, coherence et persistance. | L510 T0 S0 P0 M0 A0 V7 G0 U6
- kernel\src\fs\exofs\syscall\relation_query.rs - Stockage, VFS ExoFS, coherence et persistance. | L466 T0 S0 P0 M0 A0 V14 G0 U2
- kernel\src\fs\exofs\syscall\snapshot_create.rs - Stockage, VFS ExoFS, coherence et persistance. | L525 T0 S0 P0 M0 A0 V7 G0 U5
- kernel\src\fs\exofs\syscall\snapshot_list.rs - Stockage, VFS ExoFS, coherence et persistance. | L455 T0 S0 P0 M0 A0 V8 G0 U1
- kernel\src\fs\exofs\syscall\snapshot_mount.rs - Stockage, VFS ExoFS, coherence et persistance. | L478 T0 S0 P0 M0 A2 V4 G0 U4
- kernel\src\fs\exofs\syscall\validation.rs - Stockage, VFS ExoFS, coherence et persistance. | L680 T0 S0 P0 M0 A0 V3 G3 U12
- kernel\src\fs\exofs\tests\integration\mod.rs - Agregateur de sous-modules et exports publics. | L6 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\integration\tier_1_simple.rs - Stockage, VFS ExoFS, coherence et persistance. | L17 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\integration\tier_2_moyen.rs - Stockage, VFS ExoFS, coherence et persistance. | L36 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\integration\tier_3_stress.rs - Stockage, VFS ExoFS, coherence et persistance. | L22 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\integration\tier_4_pipeline.rs - Stockage, VFS ExoFS, coherence et persistance. | L96 T0 S0 P0 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\tests\integration\tier_5_comprehensive.rs - Stockage, VFS ExoFS, coherence et persistance. | L281 T0 S0 P0 M0 A0 V1 G0 U0
- kernel\src\fs\exofs\tests\integration\tier_6_virtio_vfs.rs - Stockage, VFS ExoFS, coherence et persistance. | L32 T0 S0 P1 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\mod.rs - Agregateur de sous-modules et exports publics. | L7 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\unit\mod.rs - Agregateur de sous-modules et exports publics. | L1 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\unit\test_blob_id.rs - Stockage, VFS ExoFS, coherence et persistance. | L12 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\unit\test_core.rs - Stockage, VFS ExoFS, coherence et persistance. | L25 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\unit\test_epoch_record.rs - Stockage, VFS ExoFS, coherence et persistance. | L12 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\exofs\tests\unit\test_xchacha20.rs - Stockage, VFS ExoFS, coherence et persistance. | L24 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\fs\mod.rs - Agregateur de sous-modules et exports publics. | L8 T0 S0 P0 M0 A0 V0 G0 U0

#### Module memory

- kernel\src\memory\arch_iface.rs - Briques bas niveau architecture CPU boot et interruptions. | L176 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\core\address.rs - Gestion memoire physique virtuelle DMA et protection. | L390 T0 S0 P0 M0 A0 V0 G0 U3
- kernel\src\memory\core\constants.rs - Gestion memoire physique virtuelle DMA et protection. | L207 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\core\layout.rs - Gestion memoire physique virtuelle DMA et protection. | L250 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\core\mod.rs - Agregateur de sous-modules et exports publics. | L67 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\core\types.rs - Gestion memoire physique virtuelle DMA et protection. | L881 T0 S0 P0 M0 A0 V0 G13 U6
- kernel\src\memory\cow\breaker.rs - Gestion memoire physique virtuelle DMA et protection. | L147 T0 S0 P0 M0 A13 V0 G0 U2
- kernel\src\memory\cow\mod.rs - Agregateur de sous-modules et exports publics. | L12 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\cow\tracker.rs - Gestion memoire physique virtuelle DMA et protection. | L193 T0 S0 P0 M4 A35 V0 G0 U3
- kernel\src\memory\dma\channels\affinity.rs - Gestion memoire physique virtuelle DMA et protection. | L236 T0 S0 P0 M0 A32 V0 G1 U2
- kernel\src\memory\dma\channels\channel.rs - Gestion memoire physique virtuelle DMA et protection. | L264 T0 S0 P0 M0 A31 V0 G0 U5
- kernel\src\memory\dma\channels\manager.rs - Gestion memoire physique virtuelle DMA et protection. | L329 T0 S0 P0 M4 A42 V0 G0 U3
- kernel\src\memory\dma\channels\mod.rs - Agregateur de sous-modules et exports publics. | L23 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\dma\channels\priority.rs - Gestion memoire physique virtuelle DMA et protection. | L302 T0 S0 P0 M4 A19 V0 G0 U2
- kernel\src\memory\dma\completion\handler.rs - Gestion memoire physique virtuelle DMA et protection. | L222 T0 S0 P0 M0 A39 V0 G0 U8
- kernel\src\memory\dma\completion\mod.rs - Agregateur de sous-modules et exports publics. | L19 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\dma\completion\polling.rs - Gestion memoire physique virtuelle DMA et protection. | L214 T0 S0 P0 M0 A20 V0 G0 U2
- kernel\src\memory\dma\completion\wakeup.rs - Gestion memoire physique virtuelle DMA et protection. | L277 T0 S0 P0 M0 A42 V0 G0 U4
- kernel\src\memory\dma\core\descriptor.rs - Gestion memoire physique virtuelle DMA et protection. | L327 T0 S0 P0 M3 A16 V0 G0 U6
- kernel\src\memory\dma\core\error.rs - Gestion memoire physique virtuelle DMA et protection. | L279 T0 S0 P0 M0 A46 V0 G0 U1
- kernel\src\memory\dma\core\mapping.rs - Gestion memoire physique virtuelle DMA et protection. | L276 T0 S0 P0 M4 A28 V0 G0 U4
- kernel\src\memory\dma\core\mod.rs - Agregateur de sous-modules et exports publics. | L28 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\dma\core\types.rs - Gestion memoire physique virtuelle DMA et protection. | L220 T0 S0 P0 M0 A4 V0 G0 U0
- kernel\src\memory\dma\core\wakeup_iface.rs - Gestion memoire physique virtuelle DMA et protection. | L153 T0 S0 P0 M0 A20 V0 G0 U3
- kernel\src\memory\dma\engines\ahci_dma.rs - Gestion memoire physique virtuelle DMA et protection. | L461 T0 S0 P0 M5 A21 V0 G0 U16
- kernel\src\memory\dma\engines\idxd.rs - Gestion memoire physique virtuelle DMA et protection. | L370 T0 S0 P0 M4 A24 V0 G0 U10
- kernel\src\memory\dma\engines\ioat.rs - Gestion memoire physique virtuelle DMA et protection. | L356 T0 S0 P0 M4 A22 V0 G0 U13
- kernel\src\memory\dma\engines\mod.rs - Agregateur de sous-modules et exports publics. | L36 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\dma\engines\nvme_dma.rs - Gestion memoire physique virtuelle DMA et protection. | L444 T0 S0 P0 M4 A23 V0 G0 U14
- kernel\src\memory\dma\engines\virtio_dma.rs - Gestion memoire physique virtuelle DMA et protection. | L372 T0 S0 P0 M4 A20 V0 G0 U10
- kernel\src\memory\dma\iommu\amd_iommu.rs - Gestion memoire physique virtuelle DMA et protection. | L293 T0 S0 P0 M0 A22 V0 G0 U9
- kernel\src\memory\dma\iommu\arm_smmu.rs - Gestion memoire physique virtuelle DMA et protection. | L463 T1 S0 P0 M6 A10 V0 G0 U8
- kernel\src\memory\dma\iommu\domain.rs - Gestion memoire physique virtuelle DMA et protection. | L297 T0 S0 P0 M7 A40 V0 G2 U5
- kernel\src\memory\dma\iommu\intel_vtd.rs - Gestion memoire physique virtuelle DMA et protection. | L386 T0 S0 P0 M0 A24 V0 G0 U14
- kernel\src\memory\dma\iommu\mod.rs - Agregateur de sous-modules et exports publics. | L19 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\dma\iommu\page_table.rs - Gestion memoire physique virtuelle DMA et protection. | L245 T0 S0 P0 M0 A0 V0 G2 U7
- kernel\src\memory\dma\mod.rs - Agregateur de sous-modules et exports publics. | L70 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\dma\ops\cyclic.rs - Gestion memoire physique virtuelle DMA et protection. | L322 T0 S0 P0 M3 A29 V0 G0 U2
- kernel\src\memory\dma\ops\interleaved.rs - Gestion memoire physique virtuelle DMA et protection. | L332 T0 S0 P0 M3 A36 V0 G0 U3
- kernel\src\memory\dma\ops\memcpy.rs - Gestion memoire physique virtuelle DMA et protection. | L116 T0 S0 P0 M0 A0 V0 G0 U3
- kernel\src\memory\dma\ops\memset.rs - Gestion memoire physique virtuelle DMA et protection. | L199 T0 S0 P0 M0 A21 V0 G0 U5
- kernel\src\memory\dma\ops\mod.rs - Agregateur de sous-modules et exports publics. | L21 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\dma\ops\scatter_gather.rs - Gestion memoire physique virtuelle DMA et protection. | L125 T0 S0 P0 M0 A0 V0 G0 U2
- kernel\src\memory\dma\stats\counters.rs - Gestion memoire physique virtuelle DMA et protection. | L252 T0 S0 P0 M0 A60 V0 G0 U1
- kernel\src\memory\dma\stats\mod.rs - Agregateur de sous-modules et exports publics. | L12 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\heap\allocator\global.rs - Gestion memoire physique virtuelle DMA et protection. | L73 T0 S0 P0 M0 A0 V0 G0 U5
- kernel\src\memory\heap\allocator\hybrid.rs - Gestion memoire physique virtuelle DMA et protection. | L145 T0 S0 P0 M0 A32 V0 G0 U1
- kernel\src\memory\heap\allocator\mod.rs - Agregateur de sous-modules et exports publics. | L16 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\heap\allocator\size_classes.rs - Gestion memoire physique virtuelle DMA et protection. | L80 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\heap\large\mod.rs - Agregateur de sous-modules et exports publics. | L7 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\heap\large\vmalloc.rs - Gestion memoire physique virtuelle DMA et protection. | L265 T0 S0 P0 M0 A27 V0 G0 U6
- kernel\src\memory\heap\mod.rs - Agregateur de sous-modules et exports publics. | L28 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\heap\thread_local\cache.rs - Gestion memoire physique virtuelle DMA et protection. | L334 T0 S0 P0 M3 A22 V0 G0 U10
- kernel\src\memory\heap\thread_local\drain.rs - Gestion memoire physique virtuelle DMA et protection. | L177 T0 S0 P0 M0 A20 V0 G0 U10
- kernel\src\memory\heap\thread_local\magazine.rs - Gestion memoire physique virtuelle DMA et protection. | L113 T0 S0 P0 M0 A0 V0 G0 U2
- kernel\src\memory\heap\thread_local\mod.rs - Agregateur de sous-modules et exports publics. | L18 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\huge_pages\hugetlbfs.rs - Gestion memoire physique virtuelle DMA et protection. | L341 T0 S0 P0 M3 A27 V0 G0 U6
- kernel\src\memory\huge_pages\mod.rs - Agregateur de sous-modules et exports publics. | L25 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\huge_pages\split.rs - Gestion memoire physique virtuelle DMA et protection. | L237 T0 S0 P0 M0 A12 V0 G0 U2
- kernel\src\memory\huge_pages\thp.rs - Gestion memoire physique virtuelle DMA et protection. | L147 T0 S0 P0 M2 A24 V0 G0 U2
- kernel\src\memory\integrity\canary.rs - Gestion memoire physique virtuelle DMA et protection. | L225 T0 S0 P0 M0 A27 V0 G0 U8
- kernel\src\memory\integrity\guard_pages.rs - Gestion memoire physique virtuelle DMA et protection. | L340 T0 S0 P0 M3 A19 V0 G0 U3
- kernel\src\memory\integrity\mod.rs - Agregateur de sous-modules et exports publics. | L50 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\integrity\sanitizer.rs - Gestion memoire physique virtuelle DMA et protection. | L321 T0 S0 P0 M0 A29 V0 G0 U11
- kernel\src\memory\mod.rs - Agregateur de sous-modules et exports publics. | L180 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\numa.rs - Gestion memoire physique virtuelle DMA et protection. | L13 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\physical\allocator\bitmap.rs - Gestion memoire physique virtuelle DMA et protection. | L186 T0 S0 P0 M3 A7 V0 G0 U3
- kernel\src\memory\physical\allocator\buddy.rs - Gestion memoire physique virtuelle DMA et protection. | L897 T0 S0 P0 M4 A49 V0 G0 U23
- kernel\src\memory\physical\allocator\mod.rs - Agregateur de sous-modules et exports publics. | L119 T0 S0 P0 M0 A0 V0 G0 U4
- kernel\src\memory\physical\allocator\numa_aware.rs - Gestion memoire physique virtuelle DMA et protection. | L306 T0 S1 P0 M4 A29 V0 G2 U1
- kernel\src\memory\physical\allocator\numa_hints.rs - Gestion memoire physique virtuelle DMA et protection. | L166 T0 S0 P0 M0 A11 V0 G0 U1
- kernel\src\memory\physical\allocator\slab.rs - Gestion memoire physique virtuelle DMA et protection. | L501 T0 S0 P1 M4 A44 V0 G0 U18
- kernel\src\memory\physical\allocator\slub.rs - Gestion memoire physique virtuelle DMA et protection. | L362 T0 S0 P0 M4 A33 V0 G0 U15
- kernel\src\memory\physical\frame\descriptor.rs - Gestion memoire physique virtuelle DMA et protection. | L482 T0 S0 P0 M0 A52 V0 G0 U6
- kernel\src\memory\physical\frame\emergency_pool.rs - Gestion memoire physique virtuelle DMA et protection. | L569 T0 S0 P0 M0 A74 V0 G0 U21
- kernel\src\memory\physical\frame\mod.rs - Agregateur de sous-modules et exports publics. | L24 T0 S0 P0 M0 A1 V0 G0 U0
- kernel\src\memory\physical\frame\pool.rs - Gestion memoire physique virtuelle DMA et protection. | L332 T1 S0 P0 M0 A55 V0 G1 U9
- kernel\src\memory\physical\frame\reclaim.rs - Gestion memoire physique virtuelle DMA et protection. | L439 T0 S0 P0 M5 A31 V0 G0 U2
- kernel\src\memory\physical\frame\ref_count.rs - Gestion memoire physique virtuelle DMA et protection. | L207 T0 S0 P0 M0 A27 V0 G0 U2
- kernel\src\memory\physical\mod.rs - Agregateur de sous-modules et exports publics. | L44 T0 S0 P0 M0 A1 V0 G0 U0
- kernel\src\memory\physical\numa\distance.rs - Gestion memoire physique virtuelle DMA et protection. | L164 T0 S0 P0 M0 A3 V0 G0 U2
- kernel\src\memory\physical\numa\migration.rs - Gestion memoire physique virtuelle DMA et protection. | L240 T0 S0 P0 M0 A22 V0 G2 U5
- kernel\src\memory\physical\numa\mod.rs - Agregateur de sous-modules et exports publics. | L50 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\physical\numa\node.rs - Gestion memoire physique virtuelle DMA et protection. | L324 T0 S0 P0 M0 A51 V0 G0 U7
- kernel\src\memory\physical\numa\policy.rs - Gestion memoire physique virtuelle DMA et protection. | L232 T0 S0 P0 M3 A16 V0 G0 U1
- kernel\src\memory\physical\stats.rs - Gestion memoire physique virtuelle DMA et protection. | L44 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\physical\zone\dma.rs - Gestion memoire physique virtuelle DMA et protection. | L56 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\physical\zone\dma32.rs - Gestion memoire physique virtuelle DMA et protection. | L51 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\physical\zone\high.rs - Gestion memoire physique virtuelle DMA et protection. | L36 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\physical\zone\mod.rs - Agregateur de sous-modules et exports publics. | L210 T0 S0 P0 M0 A19 V0 G0 U0
- kernel\src\memory\physical\zone\movable.rs - Gestion memoire physique virtuelle DMA et protection. | L58 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\physical\zone\normal.rs - Gestion memoire physique virtuelle DMA et protection. | L48 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\protection\mod.rs - Agregateur de sous-modules et exports publics. | L68 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\protection\nx.rs - Gestion memoire physique virtuelle DMA et protection. | L293 T0 S0 P0 M0 A25 V0 G1 U7
- kernel\src\memory\protection\pku.rs - Gestion memoire physique virtuelle DMA et protection. | L386 T0 S0 P0 M3 A19 V0 G0 U13
- kernel\src\memory\protection\smap.rs - Gestion memoire physique virtuelle DMA et protection. | L262 T0 S0 P0 M0 A24 V0 G0 U16
- kernel\src\memory\protection\smep.rs - Gestion memoire physique virtuelle DMA et protection. | L217 T0 S0 P0 M0 A18 V0 G0 U11
- kernel\src\memory\swap\backend.rs - Gestion memoire physique virtuelle DMA et protection. | L217 T0 S0 P0 M0 A28 V0 G0 U7
- kernel\src\memory\swap\cluster.rs - Gestion memoire physique virtuelle DMA et protection. | L326 T0 S0 P0 M3 A19 V0 G0 U0
- kernel\src\memory\swap\compress.rs - Gestion memoire physique virtuelle DMA et protection. | L288 T0 S0 P0 M4 A29 V0 G0 U4
- kernel\src\memory\swap\mod.rs - Agregateur de sous-modules et exports publics. | L27 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\swap\policy.rs - Gestion memoire physique virtuelle DMA et protection. | L182 T0 S0 P0 M6 A16 V0 G0 U2
- kernel\src\memory\utils\futex_table.rs - Gestion memoire physique virtuelle DMA et protection. | L699 T0 S0 P0 M9 A52 V0 G0 U22
- kernel\src\memory\utils\mod.rs - Agregateur de sous-modules et exports publics. | L39 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\utils\oom_killer.rs - Gestion memoire physique virtuelle DMA et protection. | L276 T0 S0 P0 M3 A37 V0 G2 U3
- kernel\src\memory\utils\shrinker.rs - Gestion memoire physique virtuelle DMA et protection. | L251 T0 S0 P0 M3 A21 V0 G0 U2
- kernel\src\memory\virtual\address_space\kernel.rs - Gestion memoire physique virtuelle DMA et protection. | L161 T0 S0 P0 M4 A5 V0 G2 U6
- kernel\src\memory\virtual\address_space\mapper.rs - Gestion memoire physique virtuelle DMA et protection. | L146 T0 S0 P0 M0 A0 V0 G1 U5
- kernel\src\memory\virtual\address_space\mod.rs - Agregateur de sous-modules et exports publics. | L18 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\virtual\address_space\tlb.rs - Gestion memoire physique virtuelle DMA et protection. | L275 T0 S0 P0 M3 A33 V0 G0 U12
- kernel\src\memory\virtual\address_space\user.rs - Gestion memoire physique virtuelle DMA et protection. | L198 T0 S0 P0 M4 A17 V0 G1 U5
- kernel\src\memory\virtual\fault\cow.rs - Gestion memoire physique virtuelle DMA et protection. | L70 T0 S0 P0 M0 A1 V0 G1 U2
- kernel\src\memory\virtual\fault\demand_paging.rs - Gestion memoire physique virtuelle DMA et protection. | L307 T0 S0 P0 M0 A48 V0 G2 U4
- kernel\src\memory\virtual\fault\handler.rs - Gestion memoire physique virtuelle DMA et protection. | L140 T0 S0 P0 M0 A25 V0 G1 U0
- kernel\src\memory\virtual\fault\mod.rs - Agregateur de sous-modules et exports publics. | L88 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\memory\virtual\fault\swap_in.rs - Gestion memoire physique virtuelle DMA et protection. | L230 T0 S0 P0 M0 A36 V0 G1 U2
- kernel\src\memory\virtual\mmap.rs - Gestion memoire physique virtuelle DMA et protection. | L420 T0 S0 P0 M0 A12 V0 G0 U15
- kernel\src\memory\virtual\mod.rs - Agregateur de sous-modules et exports publics. | L43 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\virtual\page_table\builder.rs - Gestion memoire physique virtuelle DMA et protection. | L119 T0 S0 P0 M0 A0 V0 G1 U2
- kernel\src\memory\virtual\page_table\kpti_split.rs - Gestion memoire physique virtuelle DMA et protection. | L144 T0 S1 P0 M0 A5 V0 G0 U5
- kernel\src\memory\virtual\page_table\mod.rs - Agregateur de sous-modules et exports publics. | L19 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\virtual\page_table\walker.rs - Gestion memoire physique virtuelle DMA et protection. | L221 T0 S0 P0 M0 A0 V0 G2 U16
- kernel\src\memory\virtual\page_table\x86_64.rs - Briques bas niveau architecture CPU boot et interruptions. | L296 T0 S0 P0 M0 A0 V0 G4 U6
- kernel\src\memory\virtual\vma\cow.rs - Gestion memoire physique virtuelle DMA et protection. | L138 T0 S0 P0 M0 A16 V0 G1 U1
- kernel\src\memory\virtual\vma\descriptor.rs - Gestion memoire physique virtuelle DMA et protection. | L198 T0 S0 P0 M0 A12 V0 G0 U2
- kernel\src\memory\virtual\vma\mod.rs - Agregateur de sous-modules et exports publics. | L17 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\virtual\vma\operations.rs - Gestion memoire physique virtuelle DMA et protection. | L215 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\memory\virtual\vma\tree.rs - Gestion memoire physique virtuelle DMA et protection. | L326 T0 S0 P0 M0 A0 V0 G4 U24

#### Module arch

- kernel\src\arch\aarch64\mod.rs - Agregateur de sous-modules et exports publics. | L140 T0 S0 P1 M0 A0 V0 G0 U12
- kernel\src\arch\mod.rs - Agregateur de sous-modules et exports publics. | L61 T0 S0 P1 M0 A0 V0 G0 U1
- kernel\src\arch\time.rs - Briques bas niveau architecture CPU boot et interruptions. | L15 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\acpi\hpet.rs - Briques bas niveau architecture CPU boot et interruptions. | L242 T1 S0 P0 M0 A20 V0 G0 U10
- kernel\src\arch\x86_64\acpi\madt.rs - Briques bas niveau architecture CPU boot et interruptions. | L223 T0 S0 P0 M0 A5 V0 G0 U21
- kernel\src\arch\x86_64\acpi\mod.rs - Agregateur de sous-modules et exports publics. | L17 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\acpi\parser.rs - Briques bas niveau architecture CPU boot et interruptions. | L254 T0 S0 P0 M0 A5 V0 G0 U21
- kernel\src\arch\x86_64\acpi\pm_timer.rs - Briques bas niveau architecture CPU boot et interruptions. | L183 T0 S0 P0 M0 A23 V0 G0 U3
- kernel\src\arch\x86_64\apic\io_apic.rs - Briques bas niveau architecture CPU boot et interruptions. | L213 T0 S0 P0 M0 A23 V0 G0 U9
- kernel\src\arch\x86_64\apic\ipi.rs - Briques bas niveau architecture CPU boot et interruptions. | L166 T0 S0 P0 M0 A36 V0 G0 U0
- kernel\src\arch\x86_64\apic\local_apic.rs - Briques bas niveau architecture CPU boot et interruptions. | L298 T0 S0 P0 M0 A23 V0 G0 U5
- kernel\src\arch\x86_64\apic\mod.rs - Agregateur de sous-modules et exports publics. | L113 T0 S0 P0 M0 A6 V0 G0 U1
- kernel\src\arch\x86_64\apic\x2apic.rs - Briques bas niveau architecture CPU boot et interruptions. | L167 T0 S0 P0 M0 A0 V0 G0 U7
- kernel\src\arch\x86_64\boot\early_init.rs - Briques bas niveau architecture CPU boot et interruptions. | L264 T0 S0 P0 M0 A0 V0 G0 U2
- kernel\src\arch\x86_64\boot\memory_map.rs - Briques bas niveau architecture CPU boot et interruptions. | L631 T0 S0 P0 M0 A0 V0 G0 U5
- kernel\src\arch\x86_64\boot\mod.rs - Agregateur de sous-modules et exports publics. | L28 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\boot\multiboot2.rs - Briques bas niveau architecture CPU boot et interruptions. | L171 T0 S0 P0 M0 A0 V0 G0 U7
- kernel\src\arch\x86_64\boot\trampoline_asm.rs - Briques bas niveau architecture CPU boot et interruptions. | L176 T0 S0 P1 M0 A0 V0 G0 U6
- kernel\src\arch\x86_64\boot\uefi.rs - Briques bas niveau architecture CPU boot et interruptions. | L130 T0 S1 P0 M0 A0 V0 G0 U1
- kernel\src\arch\x86_64\cpu\features.rs - Briques bas niveau architecture CPU boot et interruptions. | L515 T0 S0 P0 M0 A10 V0 G0 U6
- kernel\src\arch\x86_64\cpu\fpu.rs - Briques bas niveau architecture CPU boot et interruptions. | L472 T0 S0 P0 M0 A5 V0 G0 U32
- kernel\src\arch\x86_64\cpu\mod.rs - Agregateur de sous-modules et exports publics. | L16 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\cpu\msr.rs - Briques bas niveau architecture CPU boot et interruptions. | L316 T0 S0 P0 M0 A8 V0 G0 U16
- kernel\src\arch\x86_64\cpu\topology.rs - Briques bas niveau architecture CPU boot et interruptions. | L408 T0 S0 P0 M0 A33 V0 G0 U12
- kernel\src\arch\x86_64\cpu\tsc.rs - Briques bas niveau architecture CPU boot et interruptions. | L537 T0 S0 P0 M0 A38 V0 G0 U11
- kernel\src\arch\x86_64\exceptions.rs - Briques bas niveau architecture CPU boot et interruptions. | L867 T0 S21 P0 M0 A49 V0 G0 U31
- kernel\src\arch\x86_64\gdt.rs - Briques bas niveau architecture CPU boot et interruptions. | L297 T0 S0 P0 M0 A5 V0 G0 U6
- kernel\src\arch\x86_64\idt.rs - Briques bas niveau architecture CPU boot et interruptions. | L342 T0 S0 P0 M0 A10 V0 G0 U4
- kernel\src\arch\x86_64\memory_iface.rs - Briques bas niveau architecture CPU boot et interruptions. | L303 T0 S0 P0 M0 A4 V0 G0 U10
- kernel\src\arch\x86_64\mod.rs - Agregateur de sous-modules et exports publics. | L323 T0 S0 P0 M0 A3 V0 G0 U26
- kernel\src\arch\x86_64\paging.rs - Briques bas niveau architecture CPU boot et interruptions. | L419 T0 S0 P0 M0 A17 V0 G1 U21
- kernel\src\arch\x86_64\sched_iface.rs - Briques bas niveau architecture CPU boot et interruptions. | L168 T0 S0 P0 M0 A9 V0 G0 U2
- kernel\src\arch\x86_64\smp\hotplug.rs - Briques bas niveau architecture CPU boot et interruptions. | L129 T0 S0 P0 M0 A9 V0 G0 U2
- kernel\src\arch\x86_64\smp\init.rs - Briques bas niveau architecture CPU boot et interruptions. | L159 T0 S0 P0 M0 A9 V0 G0 U3
- kernel\src\arch\x86_64\smp\mod.rs - Agregateur de sous-modules et exports publics. | L19 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\smp\percpu.rs - Briques bas niveau architecture CPU boot et interruptions. | L230 T0 S0 P0 M0 A6 V0 G0 U13
- kernel\src\arch\x86_64\spectre\ibrs.rs - Briques bas niveau architecture CPU boot et interruptions. | L100 T0 S0 P0 M0 A16 V0 G0 U7
- kernel\src\arch\x86_64\spectre\kpti.rs - Briques bas niveau architecture CPU boot et interruptions. | L116 T0 S1 P0 M0 A16 V0 G0 U6
- kernel\src\arch\x86_64\spectre\mod.rs - Agregateur de sous-modules et exports publics. | L48 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\spectre\retpoline.rs - Briques bas niveau architecture CPU boot et interruptions. | L95 T0 S1 P0 M0 A0 V0 G0 U2
- kernel\src\arch\x86_64\spectre\ssbd.rs - Briques bas niveau architecture CPU boot et interruptions. | L63 T0 S0 P0 M0 A10 V0 G0 U3
- kernel\src\arch\x86_64\syscall.rs - Briques bas niveau architecture CPU boot et interruptions. | L320 T0 S0 P0 M0 A8 V0 G0 U6
- kernel\src\arch\x86_64\time\calibration\cpuid_nominal.rs - Briques bas niveau architecture CPU boot et interruptions. | L468 T0 S0 P0 M0 A0 V0 G0 U4
- kernel\src\arch\x86_64\time\calibration\mod.rs - Agregateur de sous-modules et exports publics. | L514 T0 S0 P0 M0 A40 V0 G0 U2
- kernel\src\arch\x86_64\time\calibration\validation.rs - Briques bas niveau architecture CPU boot et interruptions. | L427 T0 S0 P0 M0 A30 V0 G0 U3
- kernel\src\arch\x86_64\time\calibration\window.rs - Briques bas niveau architecture CPU boot et interruptions. | L553 T0 S0 P0 M0 A1 V0 G0 U12
- kernel\src\arch\x86_64\time\drift\mod.rs - Agregateur de sous-modules et exports publics. | L40 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\time\drift\periodic.rs - Briques bas niveau architecture CPU boot et interruptions. | L325 T0 S0 P6 M0 A47 V0 G0 U1
- kernel\src\arch\x86_64\time\drift\pll.rs - Briques bas niveau architecture CPU boot et interruptions. | L227 T0 S0 P0 M0 A43 V0 G0 U0
- kernel\src\arch\x86_64\time\ktime.rs - Briques bas niveau architecture CPU boot et interruptions. | L401 T0 S0 P0 M1 A73 V0 G0 U5
- kernel\src\arch\x86_64\time\mod.rs - Agregateur de sous-modules et exports publics. | L148 T0 S0 P0 M0 A0 V0 G0 U4
- kernel\src\arch\x86_64\time\percpu\mod.rs - Agregateur de sous-modules et exports publics. | L32 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\time\percpu\sync.rs - Briques bas niveau architecture CPU boot et interruptions. | L282 T0 S0 P0 M0 A30 V0 G0 U5
- kernel\src\arch\x86_64\time\sources\hpet.rs - Briques bas niveau architecture CPU boot et interruptions. | L345 T0 S0 P0 M0 A34 V0 G0 U2
- kernel\src\arch\x86_64\time\sources\mod.rs - Agregateur de sous-modules et exports publics. | L375 T0 S0 P0 M0 A16 V0 G0 U0
- kernel\src\arch\x86_64\time\sources\pit.rs - Briques bas niveau architecture CPU boot et interruptions. | L431 T0 S0 P0 M0 A16 V0 G0 U16
- kernel\src\arch\x86_64\time\sources\pm_timer.rs - Briques bas niveau architecture CPU boot et interruptions. | L335 T0 S0 P0 M0 A46 V0 G0 U1
- kernel\src\arch\x86_64\time\sources\tsc.rs - Briques bas niveau architecture CPU boot et interruptions. | L498 T0 S0 P0 M0 A27 V0 G0 U16
- kernel\src\arch\x86_64\tss.rs - Briques bas niveau architecture CPU boot et interruptions. | L302 T0 S0 P0 M0 A9 V0 G0 U13
- kernel\src\arch\x86_64\vga_early.rs - Briques bas niveau architecture CPU boot et interruptions. | L257 T1 S0 P0 M0 A22 V0 G0 U7
- kernel\src\arch\x86_64\virt\detect.rs - Briques bas niveau architecture CPU boot et interruptions. | L134 T0 S0 P0 M0 A6 V0 G0 U1
- kernel\src\arch\x86_64\virt\mod.rs - Agregateur de sous-modules et exports publics. | L12 T0 S0 P1 M0 A0 V0 G0 U0
- kernel\src\arch\x86_64\virt\paravirt.rs - Briques bas niveau architecture CPU boot et interruptions. | L56 T0 S0 P0 M0 A0 V0 G0 U2
- kernel\src\arch\x86_64\virt\stolen_time.rs - Briques bas niveau architecture CPU boot et interruptions. | L110 T0 S0 P0 M0 A8 V0 G0 U7

#### Module ipc

- kernel\src\ipc\channel\async.rs - Communication inter-processus et transport de messages. | L518 T0 S0 P0 M9 A52 V0 G0 U22
- kernel\src\ipc\channel\broadcast.rs - Communication inter-processus et transport de messages. | L637 T0 S0 P0 M8 A61 V0 G0 U24
- kernel\src\ipc\channel\mod.rs - Agregateur de sous-modules et exports publics. | L85 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\channel\mpmc.rs - Communication inter-processus et transport de messages. | L574 T0 S0 P0 M5 A73 V0 G0 U16
- kernel\src\ipc\channel\raw.rs - Communication inter-processus et transport de messages. | L406 T0 S0 P0 M5 A31 V0 G0 U2
- kernel\src\ipc\channel\streaming.rs - Communication inter-processus et transport de messages. | L525 T0 S0 P0 M4 A56 V0 G0 U19
- kernel\src\ipc\channel\sync.rs - Communication inter-processus et transport de messages. | L841 T0 S0 P0 M6 A105 V0 G0 U31
- kernel\src\ipc\channel\typed.rs - Communication inter-processus et transport de messages. | L364 T0 S0 P0 M6 A14 V0 G4 U21
- kernel\src\ipc\core\constants.rs - Communication inter-processus et transport de messages. | L187 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\core\mod.rs - Agregateur de sous-modules et exports publics. | L91 T0 S0 P0 M0 A0 V0 G0 U3
- kernel\src\ipc\core\sequence.rs - Communication inter-processus et transport de messages. | L262 T0 S0 P0 M0 A28 V0 G0 U0
- kernel\src\ipc\core\transfer.rs - Communication inter-processus et transport de messages. | L311 T0 S0 P0 M0 A1 V0 G2 U7
- kernel\src\ipc\core\types.rs - Communication inter-processus et transport de messages. | L522 T0 S0 P0 M0 A10 V0 G1 U7
- kernel\src\ipc\endpoint\connection.rs - Communication inter-processus et transport de messages. | L255 T0 S0 P0 M0 A10 V0 G0 U0
- kernel\src\ipc\endpoint\descriptor.rs - Communication inter-processus et transport de messages. | L270 T0 S0 P0 M0 A30 V0 G0 U2
- kernel\src\ipc\endpoint\lifecycle.rs - Communication inter-processus et transport de messages. | L250 T0 S0 P0 M5 A15 V0 G1 U6
- kernel\src\ipc\endpoint\mod.rs - Agregateur de sous-modules et exports publics. | L16 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\endpoint\registry.rs - Communication inter-processus et transport de messages. | L291 T0 S0 P0 M5 A8 V0 G0 U0
- kernel\src\ipc\message\builder.rs - Communication inter-processus et transport de messages. | L378 T0 S0 P0 M0 A4 V0 G0 U0
- kernel\src\ipc\message\mod.rs - Agregateur de sous-modules et exports publics. | L73 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\message\priority.rs - Communication inter-processus et transport de messages. | L472 T0 S0 P0 M0 A63 V0 G1 U9
- kernel\src\ipc\message\router.rs - Communication inter-processus et transport de messages. | L352 T0 S0 P0 M5 A44 V0 G0 U5
- kernel\src\ipc\message\serializer.rs - Communication inter-processus et transport de messages. | L329 T0 S0 P0 M0 A0 V0 G3 U4
- kernel\src\ipc\mod.rs - Agregateur de sous-modules et exports publics. | L177 T0 S0 P0 M0 A1 V0 G0 U4
- kernel\src\ipc\ring\batch.rs - Communication inter-processus et transport de messages. | L185 T0 S0 P0 M0 A7 V0 G0 U0
- kernel\src\ipc\ring\fusion.rs - Communication inter-processus et transport de messages. | L207 T0 S0 P10 M0 A31 V0 G0 U0
- kernel\src\ipc\ring\mod.rs - Agregateur de sous-modules et exports publics. | L16 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\ring\mpmc.rs - Communication inter-processus et transport de messages. | L219 T0 S0 P1 M0 A20 V0 G0 U8
- kernel\src\ipc\ring\slot.rs - Communication inter-processus et transport de messages. | L217 T0 S0 P0 M0 A8 V0 G2 U6
- kernel\src\ipc\ring\spsc.rs - Communication inter-processus et transport de messages. | L371 T0 S0 P0 M0 A25 V0 G0 U11
- kernel\src\ipc\ring\zerocopy.rs - Communication inter-processus et transport de messages. | L229 T0 S0 P0 M0 A33 V0 G0 U4
- kernel\src\ipc\rpc\client.rs - Communication inter-processus et transport de messages. | L493 T0 S0 P0 M0 A58 V0 G0 U9
- kernel\src\ipc\rpc\mod.rs - Agregateur de sous-modules et exports publics. | L76 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\rpc\protocol.rs - Communication inter-processus et transport de messages. | L394 T0 S0 P0 M0 A0 V0 G0 U2
- kernel\src\ipc\rpc\raw.rs - Communication inter-processus et transport de messages. | L233 T0 S0 P0 M0 A4 V0 G2 U4
- kernel\src\ipc\rpc\server.rs - Communication inter-processus et transport de messages. | L457 T0 S0 P0 M0 A65 V0 G0 U9
- kernel\src\ipc\rpc\timeout.rs - Communication inter-processus et transport de messages. | L286 T0 S0 P0 M0 A17 V0 G0 U1
- kernel\src\ipc\shared_memory\allocator.rs - Gestion memoire physique virtuelle DMA et protection. | L282 T0 S0 P0 M0 A35 V0 G0 U0
- kernel\src\ipc\shared_memory\descriptor.rs - Gestion memoire physique virtuelle DMA et protection. | L385 T0 S0 P0 M5 A38 V0 G0 U8
- kernel\src\ipc\shared_memory\mapping.rs - Gestion memoire physique virtuelle DMA et protection. | L362 T0 S1 P0 M8 A30 V0 G0 U14
- kernel\src\ipc\shared_memory\mod.rs - Agregateur de sous-modules et exports publics. | L65 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\shared_memory\numa_aware.rs - Gestion memoire physique virtuelle DMA et protection. | L420 T0 S0 P1 M0 A50 V0 G0 U6
- kernel\src\ipc\shared_memory\page.rs - Gestion memoire physique virtuelle DMA et protection. | L219 T0 S0 P0 M0 A26 V0 G0 U4
- kernel\src\ipc\shared_memory\pool.rs - Gestion memoire physique virtuelle DMA et protection. | L247 T0 S0 P0 M0 A30 V0 G0 U1
- kernel\src\ipc\stats\counters.rs - Communication inter-processus et transport de messages. | L326 T0 S0 P0 M0 A58 V0 G1 U2
- kernel\src\ipc\stats\mod.rs - Agregateur de sous-modules et exports publics. | L11 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\sync\barrier.rs - Communication inter-processus et transport de messages. | L397 T0 S0 P0 M0 A67 V0 G0 U5
- kernel\src\ipc\sync\event.rs - Communication inter-processus et transport de messages. | L447 T0 S0 P0 M0 A77 V0 G0 U7
- kernel\src\ipc\sync\futex.rs - Communication inter-processus et transport de messages. | L241 T0 S0 P0 M0 A15 V0 G0 U5
- kernel\src\ipc\sync\mod.rs - Agregateur de sous-modules et exports publics. | L106 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\ipc\sync\rendezvous.rs - Communication inter-processus et transport de messages. | L543 T0 S0 P0 M0 A92 V0 G0 U14
- kernel\src\ipc\sync\sched_hooks.rs - Planification, politiques execution et orchestration CPU. | L196 T0 S0 P0 M6 A0 V0 G0 U5
- kernel\src\ipc\sync\wait_queue.rs - Communication inter-processus et transport de messages. | L381 T0 S0 P0 M0 A60 V0 G0 U5

#### Module process

- kernel\src\process\auxv.rs - Gestion processus threads et cycle de vie. | L236 T0 S0 P0 M0 A0 V4 G0 U1
- kernel\src\process\core\mod.rs - Agregateur de sous-modules et exports publics. | L14 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\core\pcb.rs - Gestion processus threads et cycle de vie. | L580 T0 S0 P0 M8 A69 V2 G0 U3
- kernel\src\process\core\pid.rs - Gestion processus threads et cycle de vie. | L308 T0 S0 P0 M0 A44 V0 G1 U7
- kernel\src\process\core\registry.rs - Gestion processus threads et cycle de vie. | L252 T0 S0 P0 M5 A34 V0 G2 U11
- kernel\src\process\core\tcb.rs - Gestion processus threads et cycle de vie. | L284 T0 S0 P0 M0 A13 V0 G0 U8
- kernel\src\process\group\job_control.rs - Gestion processus threads et cycle de vie. | L165 T0 S0 P0 M0 A14 V0 G0 U1
- kernel\src\process\group\mod.rs - Agregateur de sous-modules et exports publics. | L11 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\group\pgrp.rs - Gestion processus threads et cycle de vie. | L168 T0 S0 P0 M3 A22 V0 G0 U2
- kernel\src\process\group\session.rs - Gestion processus threads et cycle de vie. | L145 T0 S0 P0 M4 A24 V0 G0 U2
- kernel\src\process\lifecycle\create.rs - Gestion processus threads et cycle de vie. | L291 T0 S0 P0 M0 A0 V0 G1 U7
- kernel\src\process\lifecycle\exec.rs - Gestion processus threads et cycle de vie. | L251 T0 S0 P0 M0 A7 V0 G0 U2
- kernel\src\process\lifecycle\exit.rs - Gestion processus threads et cycle de vie. | L153 T0 S0 P0 M0 A4 V1 G0 U4
- kernel\src\process\lifecycle\fork.rs - Gestion processus threads et cycle de vie. | L307 T0 S0 P0 M0 A7 V0 G1 U5
- kernel\src\process\lifecycle\mod.rs - Agregateur de sous-modules et exports publics. | L17 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\lifecycle\reap.rs - Gestion processus threads et cycle de vie. | L172 T0 S0 P0 M0 A20 V0 G0 U5
- kernel\src\process\lifecycle\wait.rs - Gestion processus threads et cycle de vie. | L220 T0 S0 P0 M3 A5 V0 G0 U2
- kernel\src\process\mod.rs - Agregateur de sous-modules et exports publics. | L102 T0 S0 P0 M0 A1 V0 G0 U2
- kernel\src\process\namespace\mod.rs - Agregateur de sous-modules et exports publics. | L37 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\namespace\mount_ns.rs - Gestion processus threads et cycle de vie. | L43 T0 S0 P0 M0 A9 V0 G0 U1
- kernel\src\process\namespace\net_ns.rs - Gestion processus threads et cycle de vie. | L43 T0 S0 P0 M0 A9 V0 G0 U1
- kernel\src\process\namespace\pid_ns.rs - Gestion processus threads et cycle de vie. | L116 T0 S0 P0 M4 A22 V0 G0 U2
- kernel\src\process\namespace\user_ns.rs - Gestion processus threads et cycle de vie. | L104 T0 S0 P0 M0 A13 V0 G0 U1
- kernel\src\process\namespace\uts_ns.rs - Gestion processus threads et cycle de vie. | L72 T0 S0 P0 M3 A13 V0 G0 U3
- kernel\src\process\resource\cgroup.rs - Gestion processus threads et cycle de vie. | L228 T0 S0 P0 M5 A37 V0 G0 U3
- kernel\src\process\resource\mod.rs - Agregateur de sous-modules et exports publics. | L11 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\resource\rlimit.rs - Gestion processus threads et cycle de vie. | L113 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\process\resource\usage.rs - Gestion processus threads et cycle de vie. | L146 T0 S0 P0 M0 A59 V0 G0 U0
- kernel\src\process\signal\default.rs - Gestion processus threads et cycle de vie. | L262 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\process\signal\delivery.rs - Gestion processus threads et cycle de vie. | L364 T0 S0 P0 M0 A7 V0 G0 U3
- kernel\src\process\signal\handler.rs - Gestion processus threads et cycle de vie. | L389 T0 S1 P0 M0 A4 V0 G0 U5
- kernel\src\process\signal\mask.rs - Gestion processus threads et cycle de vie. | L203 T0 S0 P0 M1 A13 V0 G1 U0
- kernel\src\process\signal\mod.rs - Agregateur de sous-modules et exports publics. | L17 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\signal\queue.rs - Gestion processus threads et cycle de vie. | L268 T0 S0 P0 M0 A31 V0 G0 U6
- kernel\src\process\signal\tcb.rs - Gestion processus threads et cycle de vie. | L277 T0 S0 P0 M0 A35 V0 G0 U0
- kernel\src\process\state\mod.rs - Agregateur de sous-modules et exports publics. | L9 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\state\transitions.rs - Gestion processus threads et cycle de vie. | L66 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\state\wakeup.rs - Gestion processus threads et cycle de vie. | L200 T0 S0 P0 M0 A18 V0 G0 U2
- kernel\src\process\thread\creation.rs - Gestion processus threads et cycle de vie. | L193 T0 S0 P0 M0 A3 V0 G0 U5
- kernel\src\process\thread\detach.rs - Gestion processus threads et cycle de vie. | L46 T0 S0 P0 M0 A3 V0 G0 U1
- kernel\src\process\thread\join.rs - Gestion processus threads et cycle de vie. | L77 T0 S0 P0 M0 A4 V0 G0 U2
- kernel\src\process\thread\local_storage.rs - Stockage, VFS ExoFS, coherence et persistance. | L212 T0 S0 P0 M3 A5 V0 G0 U6
- kernel\src\process\thread\mod.rs - Agregateur de sous-modules et exports publics. | L17 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\process\thread\pthread_compat.rs - Gestion processus threads et cycle de vie. | L276 T0 S0 P0 M0 A24 V0 G0 U7

#### Module security

- kernel\src\security\access_control\checker.rs - Controles securite, integrite et politiques de confiance. | L150 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\access_control\mod.rs - Agregateur de sous-modules et exports publics. | L34 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\access_control\object_types.rs - Controles securite, integrite et politiques de confiance. | L70 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\audit\logger.rs - Controles securite, integrite et politiques de confiance. | L322 T0 S0 P0 M2 A35 V0 G0 U3
- kernel\src\security\audit\mod.rs - Agregateur de sous-modules et exports publics. | L51 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\audit\rules.rs - Controles securite, integrite et politiques de confiance. | L298 T0 S0 P0 M2 A9 V0 G0 U0
- kernel\src\security\audit\syscall_audit.rs - Controles securite, integrite et politiques de confiance. | L309 T0 S0 P0 M2 A21 V0 G0 U1
- kernel\src\security\capability\delegation.rs - Controles securite, integrite et politiques de confiance. | L179 T0 S0 P0 M0 A0 V0 G1 U0
- kernel\src\security\capability\mod.rs - Agregateur de sous-modules et exports publics. | L212 T0 S1 P0 M3 A9 V0 G0 U0
- kernel\src\security\capability\namespace.rs - Controles securite, integrite et politiques de confiance. | L199 T0 S0 P0 M0 A13 V0 G0 U2
- kernel\src\security\capability\revocation.rs - Controles securite, integrite et politiques de confiance. | L51 T0 S0 P0 M0 A2 V0 G0 U0
- kernel\src\security\capability\rights.rs - Controles securite, integrite et politiques de confiance. | L327 T0 S0 P0 M0 A0 V0 G2 U0
- kernel\src\security\capability\table.rs - Controles securite, integrite et politiques de confiance. | L382 T0 S0 P0 M6 A55 V0 G0 U4
- kernel\src\security\capability\token.rs - Controles securite, integrite et politiques de confiance. | L334 T0 S0 P0 M0 A13 V0 G0 U0
- kernel\src\security\capability\verify.rs - Controles securite, integrite et politiques de confiance. | L204 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\crypto\aes_gcm.rs - Controles securite, integrite et politiques de confiance. | L85 T0 S1 P0 M0 A0 V0 G0 U0
- kernel\src\security\crypto\blake3.rs - Controles securite, integrite et politiques de confiance. | L201 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\crypto\ed25519.rs - Controles securite, integrite et politiques de confiance. | L177 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\crypto\kdf.rs - Controles securite, integrite et politiques de confiance. | L249 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\crypto\mod.rs - Agregateur de sous-modules et exports publics. | L118 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\crypto\rng.rs - Controles securite, integrite et politiques de confiance. | L278 T0 S0 P0 M3 A5 V0 G0 U3
- kernel\src\security\crypto\x25519.rs - Controles securite, integrite et politiques de confiance. | L139 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\crypto\xchacha20_poly1305.rs - Controles securite, integrite et politiques de confiance. | L90 T0 S1 P0 M0 A0 V0 G0 U0
- kernel\src\security\exploit_mitigations\cet.rs - Controles securite, integrite et politiques de confiance. | L243 T0 S0 P0 M0 A16 V0 G0 U7
- kernel\src\security\exploit_mitigations\cfg.rs - Controles securite, integrite et politiques de confiance. | L203 T0 S0 P0 M2 A22 V0 G0 U0
- kernel\src\security\exploit_mitigations\kaslr.rs - Controles securite, integrite et politiques de confiance. | L181 T0 S0 P0 M0 A21 V0 G0 U0
- kernel\src\security\exploit_mitigations\mod.rs - Agregateur de sous-modules et exports publics. | L109 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\security\exploit_mitigations\safe_stack.rs - Controles securite, integrite et politiques de confiance. | L298 T0 S0 P0 M2 A22 V0 G0 U1
- kernel\src\security\exploit_mitigations\stack_protector.rs - Controles securite, integrite et politiques de confiance. | L239 T0 S0 P0 M2 A20 V0 G0 U0
- kernel\src\security\integrity_check\code_signing.rs - Controles securite, integrite et politiques de confiance. | L268 T0 S0 P1 M2 A17 V0 G0 U0
- kernel\src\security\integrity_check\mod.rs - Agregateur de sous-modules et exports publics. | L48 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\integrity_check\runtime_check.rs - Controles securite, integrite et politiques de confiance. | L219 T0 S0 P0 M2 A15 V0 G0 U7
- kernel\src\security\integrity_check\secure_boot.rs - Briques bas niveau architecture CPU boot et interruptions. | L249 T0 S0 P0 M4 A11 V0 G0 U0
- kernel\src\security\isolation\domains.rs - Controles securite, integrite et politiques de confiance. | L271 T0 S0 P0 M0 A28 V0 G0 U0
- kernel\src\security\isolation\mod.rs - Agregateur de sous-modules et exports publics. | L25 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\isolation\namespaces.rs - Controles securite, integrite et politiques de confiance. | L315 T0 S0 P0 M3 A22 V0 G0 U0
- kernel\src\security\isolation\pledge.rs - Controles securite, integrite et politiques de confiance. | L260 T0 S0 P0 M0 A5 V0 G0 U0
- kernel\src\security\isolation\sandbox.rs - Controles securite, integrite et politiques de confiance. | L287 T0 S0 P0 M0 A14 V0 G0 U0
- kernel\src\security\mod.rs - Agregateur de sous-modules et exports publics. | L247 T0 S0 P0 M0 A6 V0 G0 U0
- kernel\src\security\zero_trust\context.rs - Planification, politiques execution et orchestration CPU. | L247 T0 S0 P0 M0 A29 V0 G0 U2
- kernel\src\security\zero_trust\labels.rs - Controles securite, integrite et politiques de confiance. | L210 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\zero_trust\mod.rs - Agregateur de sous-modules et exports publics. | L22 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\security\zero_trust\policy.rs - Planification, politiques execution et orchestration CPU. | L270 T0 S0 P0 M0 A16 V0 G1 U0
- kernel\src\security\zero_trust\verify.rs - Controles securite, integrite et politiques de confiance. | L178 T0 S0 P0 M0 A0 V0 G0 U0

#### Module scheduler

- kernel\src\scheduler\core\mod.rs - Agregateur de sous-modules et exports publics. | L19 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\core\pick_next.rs - Planification, politiques execution et orchestration CPU. | L151 T0 S0 P0 M0 A14 V0 G0 U2
- kernel\src\scheduler\core\preempt.rs - Planification, politiques execution et orchestration CPU. | L255 T0 S0 P0 M0 A13 V0 G0 U4
- kernel\src\scheduler\core\runqueue.rs - Planification, politiques execution et orchestration CPU. | L671 T0 S0 P0 M0 A39 V0 G0 U21
- kernel\src\scheduler\core\switch.rs - Planification, politiques execution et orchestration CPU. | L325 T2 S0 P0 M0 A8 V0 G0 U7
- kernel\src\scheduler\core\task.rs - Planification, politiques execution et orchestration CPU. | L545 T0 S0 P0 M0 A70 V0 G0 U4
- kernel\src\scheduler\energy\c_states.rs - Planification, politiques execution et orchestration CPU. | L170 T0 S0 P0 M0 A15 V0 G0 U2
- kernel\src\scheduler\energy\frequency.rs - Planification, politiques execution et orchestration CPU. | L100 T0 S0 P0 M0 A14 V0 G0 U3
- kernel\src\scheduler\energy\mod.rs - Agregateur de sous-modules et exports publics. | L9 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\energy\power_profile.rs - Planification, politiques execution et orchestration CPU. | L34 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\fpu\lazy.rs - Planification, politiques execution et orchestration CPU. | L183 T0 S0 P0 M0 A5 V0 G0 U7
- kernel\src\scheduler\fpu\mod.rs - Agregateur de sous-modules et exports publics. | L13 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\fpu\save_restore.rs - Planification, politiques execution et orchestration CPU. | L169 T0 S0 P0 M0 A7 V0 G0 U5
- kernel\src\scheduler\fpu\state.rs - Planification, politiques execution et orchestration CPU. | L147 T0 S0 P0 M0 A5 V0 G0 U2
- kernel\src\scheduler\mod.rs - Agregateur de sous-modules et exports publics. | L122 T0 S0 P0 M0 A0 V0 G0 U2
- kernel\src\scheduler\policies\cfs.rs - Planification, politiques execution et orchestration CPU. | L120 T0 S0 P0 M0 A11 V0 G0 U0
- kernel\src\scheduler\policies\deadline.rs - Planification, politiques execution et orchestration CPU. | L160 T0 S0 P0 M0 A22 V0 G0 U0
- kernel\src\scheduler\policies\idle.rs - Planification, politiques execution et orchestration CPU. | L87 T0 S0 P0 M0 A12 V0 G0 U3
- kernel\src\scheduler\policies\mod.rs - Agregateur de sous-modules et exports publics. | L11 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\policies\realtime.rs - Planification, politiques execution et orchestration CPU. | L88 T0 S0 P0 M0 A7 V0 G0 U0
- kernel\src\scheduler\smp\affinity.rs - Briques bas niveau architecture CPU boot et interruptions. | L124 T0 S0 P0 M0 A3 V0 G0 U0
- kernel\src\scheduler\smp\load_balance.rs - Briques bas niveau architecture CPU boot et interruptions. | L159 T0 S0 P0 M0 A11 V0 G0 U2
- kernel\src\scheduler\smp\migration.rs - Briques bas niveau architecture CPU boot et interruptions. | L148 T0 S0 P0 M0 A30 V0 G0 U3
- kernel\src\scheduler\smp\mod.rs - Agregateur de sous-modules et exports publics. | L11 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\smp\topology.rs - Briques bas niveau architecture CPU boot et interruptions. | L126 T0 S0 P0 M0 A9 V0 G0 U7
- kernel\src\scheduler\stats\latency.rs - Planification, politiques execution et orchestration CPU. | L132 T0 S0 P0 M0 A29 V0 G0 U1
- kernel\src\scheduler\stats\mod.rs - Agregateur de sous-modules et exports publics. | L7 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\stats\per_cpu.rs - Planification, politiques execution et orchestration CPU. | L88 T0 S0 P0 M0 A25 V0 G0 U0
- kernel\src\scheduler\sync\barrier.rs - Planification, politiques execution et orchestration CPU. | L42 T0 S0 P0 M0 A13 V0 G0 U0
- kernel\src\scheduler\sync\condvar.rs - Planification, politiques execution et orchestration CPU. | L166 T0 S0 P0 M0 A18 V0 G1 U5
- kernel\src\scheduler\sync\mod.rs - Agregateur de sous-modules et exports publics. | L17 T0 S0 P0 M2 A0 V0 G0 U0
- kernel\src\scheduler\sync\mutex.rs - Planification, politiques execution et orchestration CPU. | L191 T0 S0 P0 M2 A22 V0 G2 U7
- kernel\src\scheduler\sync\rwlock.rs - Planification, politiques execution et orchestration CPU. | L82 T0 S0 P0 M3 A10 V0 G3 U5
- kernel\src\scheduler\sync\seqlock.rs - Planification, politiques execution et orchestration CPU. | L363 T0 S0 P0 M3 A36 V0 G4 U11
- kernel\src\scheduler\sync\spinlock.rs - Planification, politiques execution et orchestration CPU. | L182 T0 S0 P0 M10 A9 V0 G4 U10
- kernel\src\scheduler\sync\wait_queue.rs - Planification, politiques execution et orchestration CPU. | L280 T0 S0 P0 M4 A10 V0 G0 U13
- kernel\src\scheduler\timer\clock.rs - Planification, politiques execution et orchestration CPU. | L177 T0 S0 P0 M0 A0 V0 G0 U4
- kernel\src\scheduler\timer\deadline_timer.rs - Planification, politiques execution et orchestration CPU. | L164 T0 S0 P0 M0 A12 V0 G0 U9
- kernel\src\scheduler\timer\hrtimer.rs - Planification, politiques execution et orchestration CPU. | L181 T0 S0 P0 M0 A7 V0 G0 U6
- kernel\src\scheduler\timer\mod.rs - Agregateur de sous-modules et exports publics. | L11 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\scheduler\timer\tick.rs - Planification, politiques execution et orchestration CPU. | L182 T0 S0 P0 M0 A30 V0 G0 U3

#### Module syscall

- kernel\src\syscall\abi.rs - Interface noyau userspace via appels systeme. | L169 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\compat\linux.rs - Interface noyau userspace via appels systeme. | L338 T0 S0 P0 M0 A13 V0 G0 U0
- kernel\src\syscall\compat\mod.rs - Agregateur de sous-modules et exports publics. | L68 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\compat\posix.rs - Interface noyau userspace via appels systeme. | L515 T1 S1 P0 M0 A8 V0 G0 U0
- kernel\src\syscall\dispatch.rs - Interface noyau userspace via appels systeme. | L706 T0 S6 P0 M0 A36 V6 G0 U17
- kernel\src\syscall\entry_asm.rs - Interface noyau userspace via appels systeme. | L142 T0 S1 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\errno.rs - Interface noyau userspace via appels systeme. | L274 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\fast_path.rs - Interface noyau userspace via appels systeme. | L497 T0 S1 P0 M1 A37 V0 G0 U9
- kernel\src\syscall\fs_bridge.rs - Stockage, VFS ExoFS, coherence et persistance. | L261 T4 S0 P0 M0 A5 V0 G0 U1
- kernel\src\syscall\handlers\fd.rs - Interface noyau userspace via appels systeme. | L153 T0 S0 P0 M0 A20 V0 G0 U0
- kernel\src\syscall\handlers\fs_posix.rs - Stockage, VFS ExoFS, coherence et persistance. | L221 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\handlers\memory.rs - Gestion memoire physique virtuelle DMA et protection. | L44 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\handlers\misc.rs - Interface noyau userspace via appels systeme. | L186 T0 S0 P0 M0 A1 V0 G0 U7
- kernel\src\syscall\handlers\mod.rs - Agregateur de sous-modules et exports publics. | L13 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\handlers\process.rs - Gestion processus threads et cycle de vie. | L288 T0 S0 P0 M0 A1 V0 G0 U9
- kernel\src\syscall\handlers\signal.rs - Interface noyau userspace via appels systeme. | L342 T0 S2 P0 M0 A4 V0 G0 U17
- kernel\src\syscall\handlers\time.rs - Interface noyau userspace via appels systeme. | L66 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\mod.rs - Agregateur de sous-modules et exports publics. | L173 T0 S0 P0 M0 A0 V0 G0 U1
- kernel\src\syscall\numbers.rs - Interface noyau userspace via appels systeme. | L470 T0 S0 P0 M0 A0 V0 G0 U0
- kernel\src\syscall\table.rs - Interface noyau userspace via appels systeme. | L752 T0 S1 P0 M0 A9 V0 G0 U5
- kernel\src\syscall\validation.rs - Interface noyau userspace via appels systeme. | L651 T0 S0 P0 M0 A20 V1 G4 U8

#### Module exophoenix

- kernel\src\exophoenix\forge.rs - Resilience, isolation et reprise pipeline ExoPhoenix. | L385 T0 S0 P12 M0 A2 V0 G1 U6
- kernel\src\exophoenix\handoff.rs - Resilience, isolation et reprise pipeline ExoPhoenix. | L316 T0 S0 P1 M0 A25 V0 G0 U11
- kernel\src\exophoenix\interrupts.rs - Resilience, isolation et reprise pipeline ExoPhoenix. | L137 T0 S0 P0 M0 A2 V0 G0 U17
- kernel\src\exophoenix\isolate.rs - Resilience, isolation et reprise pipeline ExoPhoenix. | L105 T0 S0 P3 M0 A1 V0 G0 U2
- kernel\src\exophoenix\mod.rs - Agregateur de sous-modules et exports publics. | L32 T0 S0 P0 M0 A3 V0 G0 U0
- kernel\src\exophoenix\sentinel.rs - Resilience, isolation et reprise pipeline ExoPhoenix. | L319 T0 S0 P0 M0 A13 V0 G0 U9
- kernel\src\exophoenix\ssr.rs - Resilience, isolation et reprise pipeline ExoPhoenix. | L38 T0 S0 P0 M0 A3 V0 G0 U1
- kernel\src\exophoenix\stage0.rs - Resilience, isolation et reprise pipeline ExoPhoenix. | L1157 T0 S0 P0 M0 A156 V0 G0 U20

#### Module (racine)

- kernel\src\lib.rs - Surface API globale et orchestration des modules. | L292 T1 S0 P0 M0 A0 V0 G0 U12
- kernel\src\main.rs - Point entree et initialisation du sous-systeme. | L350 T0 S0 P0 M0 A0 V0 G0 U1

### 5.2 Exo-Boot exo-boot/src

#### Module uefi

- exo-boot\src\uefi\entry.rs - Briques bas niveau architecture CPU boot et interruptions. | L133 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\uefi\exit.rs - Briques bas niveau architecture CPU boot et interruptions. | L103 T0 S0 P0 M0 A6 V0 G0 U0
- exo-boot\src\uefi\mod.rs - Agregateur de sous-modules et exports publics. | L19 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\uefi\protocols\file.rs - Briques bas niveau architecture CPU boot et interruptions. | L267 T0 S0 P0 M0 A0 V0 G2 U5
- exo-boot\src\uefi\protocols\graphics.rs - Briques bas niveau architecture CPU boot et interruptions. | L176 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\uefi\protocols\loaded_image.rs - Briques bas niveau architecture CPU boot et interruptions. | L103 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\uefi\protocols\mod.rs - Agregateur de sous-modules et exports publics. | L15 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\uefi\protocols\rng.rs - Briques bas niveau architecture CPU boot et interruptions. | L250 T0 S0 P0 M0 A0 V0 G0 U5
- exo-boot\src\uefi\secure_boot.rs - Briques bas niveau architecture CPU boot et interruptions. | L152 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\uefi\services.rs - Briques bas niveau architecture CPU boot et interruptions. | L233 T0 S0 P0 M0 A0 V0 G3 U8

#### Module kernel_loader

- exo-boot\src\kernel_loader\elf.rs - Briques bas niveau architecture CPU boot et interruptions. | L345 T0 S0 P0 M0 A0 V0 G1 U7
- exo-boot\src\kernel_loader\handoff.rs - Briques bas niveau architecture CPU boot et interruptions. | L304 T0 S0 P0 M0 A0 V0 G0 U6
- exo-boot\src\kernel_loader\mod.rs - Agregateur de sous-modules et exports publics. | L136 T0 S0 P0 M0 A0 V0 G1 U3
- exo-boot\src\kernel_loader\relocations.rs - Briques bas niveau architecture CPU boot et interruptions. | L253 T0 S0 P0 M0 A0 V0 G0 U6
- exo-boot\src\kernel_loader\verify.rs - Briques bas niveau architecture CPU boot et interruptions. | L193 T0 S0 P0 M0 A0 V0 G0 U2

#### Module memory

- exo-boot\src\memory\map.rs - Briques bas niveau architecture CPU boot et interruptions. | L377 T0 S0 P0 M0 A0 V0 G0 U1
- exo-boot\src\memory\mod.rs - Agregateur de sous-modules et exports publics. | L46 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\memory\paging.rs - Briques bas niveau architecture CPU boot et interruptions. | L343 T0 S0 P0 M0 A0 V0 G0 U13
- exo-boot\src\memory\regions.rs - Briques bas niveau architecture CPU boot et interruptions. | L251 T0 S0 P0 M0 A0 V0 G0 U10

#### Module bios

- exo-boot\src\bios\disk.rs - Briques bas niveau architecture CPU boot et interruptions. | L208 T0 S0 P0 M0 A0 V0 G0 U1
- exo-boot\src\bios\mod.rs - Agregateur de sous-modules et exports publics. | L203 T0 S0 P0 M0 A0 V0 G0 U8
- exo-boot\src\bios\vga.rs - Briques bas niveau architecture CPU boot et interruptions. | L264 T0 S0 P0 M0 A0 V0 G0 U9

#### Module display

- exo-boot\src\display\font.rs - Briques bas niveau architecture CPU boot et interruptions. | L228 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\display\framebuffer.rs - Briques bas niveau architecture CPU boot et interruptions. | L432 T0 S0 P0 M0 A30 V0 G1 U2
- exo-boot\src\display\mod.rs - Agregateur de sous-modules et exports publics. | L92 T0 S0 P0 M0 A0 V0 G0 U0

#### Module config

- exo-boot\src\config\defaults.rs - Briques bas niveau architecture CPU boot et interruptions. | L112 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\config\mod.rs - Agregateur de sous-modules et exports publics. | L78 T0 S0 P0 M0 A0 V0 G0 U0
- exo-boot\src\config\parser.rs - Briques bas niveau architecture CPU boot et interruptions. | L168 T0 S0 P0 M0 A0 V0 G0 U0

#### Module (racine)

- exo-boot\src\main.rs - Point entree et initialisation du sous-systeme. | L252 T0 S0 P0 M0 A0 V0 G0 U9
- exo-boot\src\panic.rs - Gestion de panique, fallback et diagnostics critiques. | L150 T0 S0 P0 M0 A7 V0 G0 U5

## 6. Forces manques priorites

### 6.1 Forces
- Couverture modulaire large memoire IPC FS securite scheduler architecture.
- Signal de resilience via exophoenix pipeline de reprise explicite.
- Densite notable de primitives synchro et atomiques modules concurrents.

### 6.2 Manques
- TODO stubs placeholders encore presents dans des chemins critiques.
- Zones unsafe nombreuses demandent invariants documentes et tests cibles.
- Validation inter-modules a systematiser FS Memory Security IPC.

### 6.3 Priorites
1. Fermer TODO stubs boot memory arch scheduler.
2. Ajouter matrice de tests ExoPhoenix panne reprise.
3. Renforcer audit lock ordering et data race sur zones concurrentes.
4. Stabiliser interfaces syscall et contrats API transverses.

## 7. Glossaire metriques
- L lignes fichier approximation brute.
- T occurrences TODO.
- S occurrences stub todo! unimplemented!.
- P occurrences placeholder FIXME TBD ADAPT.
- M Mutex SpinLock RwLock OnceLock.
- A atomiques et ordering.
- V occurrences Vec T.
- G signatures generiques.
- U occurrences unsafe.

---
Rapport unifie genere automatiquement pour consolidation exhaustive et deduplication.
