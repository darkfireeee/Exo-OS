# ExoNet V4 Implementation Audit

This file records the implementation mapping against:

- `docs/recast/EXOOS_NETWORK_MODULE_V4.md`
- `docs/Exo-OS-TLA+/ExoNet_StressTest.tla`

## Confirmed Structure

- `network_server` no longer depends on the removed DPDK, XDP, io_uring, `socket/`, or `stack/` modules.
- BSD socket syscalls 41-55 are routed through `kernel/src/syscall/net_bridge.rs`.
- The kernel bridge uses IPC raw RPC to `network_server`; userspace pointers are copied or decoded in-kernel before IPC.
- `NetMsg`, `NetReply`, `DriverInitMsg`, and `RxReleaseMsg` use the fixed V4 sizes.
- `RxReleaseMsg` carries exactly 20 `u16` pool indices, matching the corrected 48-byte layout.
- `network_server` owns `released_buf[64]` and flushes releases in batches to the driver.
- `virtio_net` owns `rx_submitted[]` and refills RX slots only after `RxReleaseMsg`.
- Phoenix isolation has explicit `Normal -> Draining -> Serialized -> Normal` state transitions.

## TLA Mapping

| TLA variable/action | Code location |
| --- | --- |
| `rx_submitted` | `drivers/network/virtio_net/src/net.rs` |
| `tx_inflight` / TX ownership | `drivers/network/virtio_net/src/net.rs`, `servers/network_server/src/buf_pool.rs` |
| `ipc_rx_ring`, `ipc_pkt_ring` | `servers/network_server/src/virtio_device.rs` |
| `ipc_tx_ring` / release batches | `servers/network_server/src/driver_link.rs` |
| `ns_released_buf` | `servers/network_server/src/virtio_device.rs` |
| `socket_slots` | `servers/network_server/src/socket_table.rs` |
| `NS_SendDriverInit` | `servers/network_server/src/driver_link.rs` |
| `VN_ProcessReleases` | `drivers/network/virtio_net/src/net.rs` |
| `Phoenix_*` | `servers/network_server/src/isolation.rs` |

## Stress Test

`servers/network_server/tests/exonet_stress.rs` simulates the TLA state machine with deterministic stress over RX, TX, socket allocation, release batching, and Phoenix drain/restore. It asserts:

- no RX double ownership;
- no RX leak;
- no TX double ownership;
- no TX leak;
- socket slots stay bounded by 64;
- no traffic is accepted before boot reaches ready.
