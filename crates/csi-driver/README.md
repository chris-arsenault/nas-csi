# crates/csi-driver

CSI controller service and shared CSI bootstrap.

Responsibilities:

- Implement CSI Identity and Controller services.
- Integrate with Kubernetes CSI sidecars.
- Register existing TrueNAS filesystem datasets as volumes.
- Call TrueNAS for dataset and snapshot operations.
- Call the host agent for node publish reconciliation.

Out of scope:

- Block volumes.
- NFS subdirectory provisioning.
- Direct libvirt or QEMU manipulation.

The CSI protobuf bindings should be generated from the upstream CSI spec during
the build or through `xtask`, not copied from stale generated code.
