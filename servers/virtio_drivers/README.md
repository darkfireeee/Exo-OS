# virtio_drivers service

In v0.2.0 this service is intentionally limited to lifecycle/status IPC
(`VIRTIO_MSG_HEARTBEAT`, `VIRTIO_MSG_STATUS`).

Actual block-device VirtIO work is implemented in the kernel-side driver
`drivers/storage/virtio_blk/`. New I/O paths must not assume this service owns
virtqueue descriptors until a future refactor moves the full virtio protocol to
Ring1.
