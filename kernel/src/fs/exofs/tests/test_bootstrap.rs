#[test]
fn register_storage_flush_barrier_installs_hook() {
    crate::fs::exofs::epoch::epoch_barriers::register_nvme_flush_fn(
        crate::fs::exofs::storage::virtio_adapter::flush_global_disk,
    );
    assert!(crate::fs::exofs::epoch::epoch_barriers::is_nvme_flush_registered());
}
