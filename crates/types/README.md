# crates/types

Shared Rust types for the project.

Planned contents:

- `VolumeId`, `DatasetName`, `NodeId`, and `ExportId` newtypes.
- `ClusterIntent` for repo-safe schematic intent files.
- `DiscoveryInventory` for read-only host facts.
- `HostConfig` for generated local desired state.
- `VolumePolicy` enum:
  - `ReposDev`
  - `SamplesReadOnly`
  - `Custom`
- `ClusterProfile` enum:
  - `MaintenanceBasic`
  - `MaintenanceControlPlane`
- `CachePolicy`, `LockPolicy`, and `AccessMode`.
- Host-agent request/response DTOs.
- Driver-owned error taxonomy.

This crate must stay dependency-light so it can be used by the CSI driver,
host-agent, and tests.
