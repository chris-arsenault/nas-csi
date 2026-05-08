# crates/truenas-client

Typed Rust primitives for the small TrueNAS API surface this project needs.

Current shape:

- JSON-RPC 2.0 request/response serialization.
- A transport trait for tests and future WebSocket integration.
- API-key login request support.
- Typed method wrappers for dataset, snapshot, and SMB operations currently
  used by discovery and CSI controller planning.

Implemented method wrappers:

- `system.info`
- `system.ready`
- `pool.dataset.query`
- `pool.dataset.create`
- `pool.dataset.update`
- `pool.dataset.delete`
- `pool.snapshot.query`
- `pool.snapshot.create`
- `pool.snapshot.delete`
- `sharing.smb.query`
- `sharing.smb.create`
- `sharing.smb.update`

A production WebSocket transport, retry policy, event subscription layer, and
additional TrueNAS APIs should be added only as the controller or host-agent
needs them.
