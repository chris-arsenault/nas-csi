# Component Structure

## Process Boundaries

```text
Kubernetes API
  -> CSI sidecars
  -> nas-csi-controller
       -> TrueNAS JSON-RPC API
       -> nas-csi-host-agent gRPC API

kubelet
  -> nas-csi-node-plugin
       -> Linux mount/findmnt/bind mount
       -> /etc/nas-csi/node.yaml
       -> optional nas-csi-node-agent helper

TrueNAS host
  -> nas-csi-host-agent
       -> discovery for read-only host inventory
       -> TrueNAS JSON-RPC API for datasets and SMB inventory
       -> libvirt for node VM lifecycle
       -> QEMU for node execution
       -> k3s bootstrap and cluster reconciliation
       -> virtiofsd-rs processes

Kubernetes node VM
  -> virtiofs kernel client
  -> mounted exports under /var/lib/nas-csi/virtiofs
  -> CSI staging paths under kubelet
  -> pod bind mounts under kubelet pod paths
```

## nas-csi-controller

Runs in Kubernetes as a Deployment with CSI controller sidecars.

Responsibilities:

- Implement CSI Identity and Controller services.
- Register existing TrueNAS filesystem datasets as CSI volumes.
- Optionally create new filesystem datasets for explicitly dynamic classes.
- Refuse zvol/block mode for same-dataset storage classes.
- Manage CSI snapshots by calling TrueNAS ZFS snapshot APIs.
- Call the host agent to ensure a dataset is exported to a node VM.
- Track Kubernetes volume identity in TrueNAS dataset properties or a small
  driver metadata dataset.

Non-responsibilities:

- It does not mount filesystems.
- It does not run `virtiofsd`.
- It does not edit VM XML directly.

## nas-csi-node-plugin

Runs in Kubernetes as a privileged DaemonSet on every node VM.

Responsibilities:

- Implement CSI Identity and Node services.
- Read and validate `/etc/nas-csi/node.yaml`.
- Bind mounted virtiofs exports to CSI staging paths.
- Verify the mount type and expected dataset identity.
- Bind-mount datasets or subpaths into pod target paths.
- Enforce read-only publish for read-only volume policies.
- Fail closed if a virtiofs tag is absent.

Non-responsibilities:

- It does not create datasets.
- It does not manage SMB shares.
- It does not start `virtiofsd`.

## nas-csi-host-agent

Runs directly on the TrueNAS host, outside Kubernetes.

Responsibilities:

- Expose a narrow authenticated gRPC API:
- `EnsureExport`
- `RemoveExport`
- `ListExports`
- `GetExportHealth`
- `ReconcileNode`
- `EnsureNodeVm`
- `DestroyNodeVm`
- `GetNodeVmHealth`
- `EnsureCluster`
- `GetClusterHealth`
- Start, stop, and supervise `virtiofsd-rs`.
- Own node VM domain desired state.
- Create, start, stop, restart, destroy, and rebuild node VMs.
- Render cloud-init seed images.
- Manage node root disk images.
- Bootstrap and reconcile the k3s cluster.
- Install and reconcile the Kubernetes-side `nas-csi` components.
- Own socket paths and permissions under `/run/nas-csi`.
- Ensure VM virtiofs devices exist.
- Detect when TrueNAS middleware has regenerated or restarted VM definitions.
- Cordon or mark unhealthy node exports when the transport fails.
- Emit structured logs and metrics for export health, request latency, daemon
  restarts, and file descriptor pressure.
- Optionally expose a small operational UI or CLI for host-agent-owned VM
  status, console helpers, and node actions.

The agent should be boring and explicit. It should reconcile a small desired
state file plus CSI requests, not infer broad storage behavior from the host.

## nas-csi-vm-manager

Shared Rust library used by the host agent.

Responsibilities:

- Render libvirt domain XML from typed desired state.
- Create and compare managed domain definitions through `nas-csi` libvirt
  metadata.
- Manage domain autostart, start, stop, shutdown, destroy, and undefine.
- Manage root disk image creation and expansion.
- Generate cloud-init NoCloud seed images.
- Validate bridge, firmware, machine, and CPU settings against host capability.
- Store and read `nas-csi` metadata in domain XML.

This is a library, not a separate daemon. Keeping it separate from the host
agent makes the VM IaC behavior testable without running the CSI control plane.

## nas-csi-cluster-manager

Shared Rust library used by the host agent.

Responsibilities:

- Render k3s server and agent config files.
- Render cluster-specific cloud-init fragments.
- Manage k3s token and kubeconfig files.
- Bootstrap the first server node.
- Join additional server and agent nodes.
- Wait for Kubernetes API and node readiness.
- Install or reconcile substrate add-ons, including `nas-csi`.
- Plan and execute k3s upgrades.
- Keep application workloads out of scope.

The cluster manager should support k3s first. A kubeadm backend can be added
later behind the same high-level desired-state model.

## nas-csi-discovery

Shared Rust library used by the host agent.

Responsibilities:

- Build read-only `DiscoveryInventory`.
- Detect TrueNAS, dataset, SMB, libvirt, bridge, QEMU, firmware, CPU, memory,
  image, and `virtiofsd` facts.
- Detect existing project-owned state.
- Feed `init` and `plan`.

This library must not mutate host state.

## nas-csi-truenas-client

Shared Rust library used by the controller and host agent.

Responsibilities:

- JSON-RPC 2.0 over WebSocket transport.
- API-key authentication.
- Typed wrappers for the subset of methods we use:
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
- Error normalization into driver-owned error types.
- Retry policy for transient API failures.

## virtiofsd-rs Fork

The upstream Rust `virtiofsd` should be treated as a pinned dependency we can
fork when needed.

Fork triggers:

- File descriptor retention hurts repository/package-manager workloads.
- ZFS-backed exports expose ACL/xattr behavior we need to patch or instrument.
- We need a stable health/control surface beyond process supervision.
- We need deterministic cache behavior for SMB-coexisting datasets.

Initial policy should be "configure before fork." If configuration and resource
limits are not enough, fork and keep the patch set small.

## Kubernetes Sidecars

Use the standard CSI sidecars:

- `external-provisioner` for PVC to `CreateVolume`.
- `external-attacher` only if we keep `ControllerPublishVolume`.
- `external-snapshotter` for VolumeSnapshot support.
- `node-driver-registrar` for kubelet plugin registration.
- `livenessprobe` for CSI socket health.

For early static-PV testing, the controller side can be omitted and the node
plugin can be validated first.
