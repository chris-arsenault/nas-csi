# Work Tracking

This tracks the remaining implementation work after the state-aware reconcile
slice. Verification work is intentionally excluded here.

## 1. Execute Safety

1. [x] Add atomic file writes for rendered artifacts: temp file, fsync, rename.
2. [x] Add atomic binary writes for cloud-init seed images.
3. [x] Add atomic systemd unit writes.
4. [x] Add a host-agent apply lock so two applies cannot run concurrently.
5. [x] Split dry-run output into a concise summary of applies, skips, refusals,
   and risky operations.
6. [x] Add a `status` command that reports actual state without rendering an
   apply plan.
7. [x] Add rollback or backup behavior for changed systemd unit files.
8. [x] Add rollback or backup behavior for changed libvirt domain definitions.
9. [x] Add stricter execute path guards so writes only target expected artifact,
   systemd, root-disk, and seed-image paths.

## 2. VM And Runtime

1. [x] Validate base image existence before root disk creation.
2. [x] Validate base image format before root disk creation.
3. [x] Validate base image checksum before root disk creation.
4. [x] Add root disk resize handling for existing disks smaller than desired.
5. [x] Add virtiofsd socket readiness checks after service start or restart.
6. [x] Add libvirt domain ownership checks using `nas-csi` metadata.
7. [x] Refuse to manage domains without `nas-csi` metadata unless an explicit
   adoption path is enabled.
8. [x] Add stopped-domain redefine flow.
9. [x] Keep redefined VMs stopped unless a separate start policy is enabled.

## 3. Host Agent Ownership

1. [x] Package the host agent for installation on the TrueNAS host.
2. [x] Add a host-agent systemd unit and environment file.
3. [x] Define config directory layout and permissions.
4. [x] Define secret file paths and permissions.
5. [x] Define log and runtime directory layout.
6. [x] Add structured logs for every reconcile decision.
7. [x] Add structured logs for every command execution.
8. [x] Add health output for required host tools.
9. [x] Add health output for systemd units.
10. [x] Add health output for libvirt domains.
11. [x] Add health output for virtiofs sockets.
12. [x] Add health output for mounted datasets.

## 4. Cluster / CSI

1. [x] Implement k3s first-server bootstrap orchestration after VM creation.
2. [x] Implement k3s agent/server join orchestration for additional nodes.
3. [x] Manage the k3s cluster token lifecycle.
4. [x] Retrieve kubeconfig from the initialized cluster.
5. [x] Store kubeconfig at the configured host-local path with safe permissions.
6. [x] Add cluster API readiness checks.
7. [x] Add node readiness checks.
8. [x] Add node label and taint reconciliation.
9. [x] Add cluster add-on reconciliation for substrate components.
10. [x] Install or reconcile the `nas-csi` Kubernetes manifests or Helm chart.
11. [x] Generate CSI protobuf bindings from the upstream CSI spec.
12. [x] Implement CSI controller service bootstrap.
13. [x] Implement `CreateVolume` for existing TrueNAS filesystem datasets.
14. [x] Implement optional `CreateVolume` dataset creation through the TrueNAS
   API.
15. [x] Implement `DeleteVolume` safety semantics that never delete
   authoritative datasets unless explicitly enabled.
16. [x] Implement `ControllerPublishVolume` and `ControllerUnpublishVolume`
   semantics for node/export assignment.
17. [x] Implement `ValidateVolumeCapabilities` for supported filesystem modes.
18. [x] Implement `ListVolumes`, `GetCapacity`, and controller identity calls.
19. [x] Implement SMB share metadata discovery and optional management.
20. [x] Implement snapshot discovery and snapshot lifecycle API integration.
21. [x] Implement retention/replication metadata integration where needed.
22. [x] Implement CSI node plugin gRPC server.
23. [x] Implement node-side runtime config loading from `/etc/nas-csi/node.yaml`.
24. [x] Implement `NodeStageVolume` using the virtiofs-mounted source path.
25. [x] Implement `NodePublishVolume` bind mounts into pod target paths.
26. [x] Implement `NodeUnpublishVolume` and `NodeUnstageVolume`.
27. [x] Implement node-side mount validation and fail-closed behavior.
28. [x] Add Kubernetes controller Deployment manifests.
29. [x] Add Kubernetes node DaemonSet manifests.
30. [x] Add RBAC, CSIDriver, StorageClass, and example PVC manifests.
