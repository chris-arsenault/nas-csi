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

1. [ ] Validate base image existence before root disk creation.
2. [ ] Validate base image format before root disk creation.
3. [ ] Validate base image checksum before root disk creation.
4. [ ] Add root disk resize handling for existing disks smaller than desired.
5. [ ] Add virtiofsd socket readiness checks after service start or restart.
6. [ ] Add libvirt domain ownership checks using `nas-csi` metadata.
7. [ ] Refuse to manage domains without `nas-csi` metadata unless an explicit
   adoption path is enabled.
8. [ ] Add stopped-domain redefine flow.
9. [ ] Keep redefined VMs stopped unless a separate start policy is enabled.

## 3. Host Agent Ownership

1. [ ] Package the host agent for installation on the TrueNAS host.
2. [ ] Add a host-agent systemd unit and environment file.
3. [ ] Define config directory layout and permissions.
4. [ ] Define secret file paths and permissions.
5. [ ] Define log and runtime directory layout.
6. [ ] Add structured logs for every reconcile decision.
7. [ ] Add structured logs for every command execution.
8. [ ] Add health output for required host tools.
9. [ ] Add health output for systemd units.
10. [ ] Add health output for libvirt domains.
11. [ ] Add health output for virtiofs sockets.
12. [ ] Add health output for mounted datasets.

## 4. Cluster / CSI

1. [ ] Implement k3s first-server bootstrap orchestration after VM creation.
2. [ ] Implement k3s agent/server join orchestration for additional nodes.
3. [ ] Manage the k3s cluster token lifecycle.
4. [ ] Retrieve kubeconfig from the initialized cluster.
5. [ ] Store kubeconfig at the configured host-local path with safe permissions.
6. [ ] Add cluster API readiness checks.
7. [ ] Add node readiness checks.
8. [ ] Add node label and taint reconciliation.
9. [ ] Add cluster add-on reconciliation for substrate components.
10. [ ] Install or reconcile the `nas-csi` Kubernetes manifests or Helm chart.
11. [ ] Generate CSI protobuf bindings from the upstream CSI spec.
12. [ ] Implement CSI controller service bootstrap.
13. [ ] Implement `CreateVolume` for existing TrueNAS filesystem datasets.
14. [ ] Implement optional `CreateVolume` dataset creation through the TrueNAS
   API.
15. [ ] Implement `DeleteVolume` safety semantics that never delete
   authoritative datasets unless explicitly enabled.
16. [ ] Implement `ControllerPublishVolume` and `ControllerUnpublishVolume`
   semantics for node/export assignment.
17. [ ] Implement `ValidateVolumeCapabilities` for supported filesystem modes.
18. [ ] Implement `ListVolumes`, `GetCapacity`, and controller identity calls.
19. [ ] Implement SMB share metadata discovery and optional management.
20. [ ] Implement snapshot discovery and snapshot lifecycle API integration.
21. [ ] Implement retention/replication metadata integration where needed.
22. [ ] Implement CSI node plugin gRPC server.
23. [ ] Implement node-side runtime config loading from `/etc/nas-csi/node.yaml`.
24. [ ] Implement `NodeStageVolume` using the virtiofs-mounted source path.
25. [ ] Implement `NodePublishVolume` bind mounts into pod target paths.
26. [ ] Implement `NodeUnpublishVolume` and `NodeUnstageVolume`.
27. [ ] Implement node-side mount validation and fail-closed behavior.
28. [ ] Add Kubernetes controller Deployment manifests.
29. [ ] Add Kubernetes node DaemonSet manifests.
30. [ ] Add RBAC, CSIDriver, StorageClass, and example PVC manifests.
