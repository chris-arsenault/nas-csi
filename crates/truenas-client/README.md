# crates/truenas-client

Typed Rust client for the small TrueNAS API surface this project needs.

Transport:

- JSON-RPC 2.0 over WebSocket.
- API-key login.
- Request IDs mapped to pending calls.
- Event subscription support for VM/dataset changes if reliable in the target
  TrueNAS version.

Initial method wrappers:

- `system.info`
- `system.ready`
- `pool.dataset.query`
- `pool.dataset.create`
- `pool.dataset.update`
- `pool.dataset.delete`
- `pool.snapshot.create`
- `pool.snapshot.clone`
- `pool.snapshot.delete`
- `sharing.smb.query`
- `service.query`
- `vm.query`
- `vm.device.query`

This crate should not expose generic untyped JSON calls to the rest of the
codebase except behind a clearly marked escape hatch for lab work.
