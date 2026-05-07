# Architecture

## Non-Negotiable Requirements

The target storage is the same data, not a copy and not a separate block volume.

Required properties:

- The data lives in ordinary TrueNAS ZFS filesystem datasets.
- The datasets remain visible and manageable in TrueNAS.
- TrueNAS periodic snapshots, replication, retention, and restore workflows keep
  working on those datasets.
- Some datasets are also SMB shares for other LAN hosts.
- Kubernetes workloads mount the same datasets for high-volume small-file work
  and read-only streaming workloads.

Those requirements rule out zvol-backed CSI for these datasets. A zvol can be
snapshotted and replicated by TrueNAS, but after Kubernetes formats it as `xfs`
or `ext4`, TrueNAS cannot safely SMB-share the files inside it.

## Hard Boundary

A Kubernetes node running inside a VM cannot directly access a TrueNAS
host-mounted filesystem dataset unless a real transport crosses the VM boundary.

The viable transport classes are:

- Network file protocol: SMB, NFS, or another remote filesystem.
- Hypervisor filesystem passthrough: virtiofs for a KVM/Linux guest.
- Host/container bind mount: only if Kubernetes or the workload runs directly in
  a host-level container namespace.

CSI does not change this. CSI can automate provisioning and publishing around
the transport, but it cannot make a VM bypass the hypervisor boundary.

## Primary Design: Dataset CSI Over Virtiofs

When the Kubernetes node VMs run on the TrueNAS host, the serious design is a
filesystem dataset CSI driver that uses virtiofs as the VM transport. The
preferred deployment is for `nas-csi-host-agent` to own the node VM domains
directly as IaC-managed libvirt resources.

Data path:

1. TrueNAS mounts the ZFS dataset at its normal discovered host path, such as
   `/mnt/<pool>/<dataset>`.
2. TrueNAS SMB exports that path when LAN SMB access is required.
3. The host agent defines and starts the Kubernetes node VM.
4. The VM receives the same host path through a QEMU/libvirt virtiofs device.
5. The VM mounts the virtiofs tag at a stable generated node path, such as
   `/var/lib/nas-csi/host-datasets/<volume-id>`.
6. The CSI node plugin bind-mounts the dataset, or a subpath of it, into pods.

Control path:

1. The CSI controller talks to TrueNAS over the API.
2. It discovers or creates filesystem datasets, never zvols, for this storage
   class.
3. It records Kubernetes volume identity in dataset properties or driver-owned
   metadata.
4. It optionally manages SMB share intent, but TrueNAS remains the SMB server.
5. It coordinates snapshots and restores through TrueNAS/ZFS APIs.
6. The host agent ensures the required node VM and virtiofs devices exist.

This keeps one file tree:

```text
TrueNAS ZFS dataset
  -> TrueNAS SMB service for LAN clients
  -> virtiofs device into Kubernetes node VM
  -> CSI bind mount into pod
```

## Host-Agent Ownership

The host agent is mandatory. It owns the local compute substrate for this
project: Kubernetes node VM lifecycle plus virtiofs transport. TrueNAS remains
the storage appliance and data-protection authority.

TrueNAS remains the source of truth for:

- ZFS filesystem datasets.
- SMB shares.
- snapshots, snapshot tasks, replication, quotas, and retention.

The host agent becomes the source of truth for:

- node VM desired state;
- k3s cluster desired state;
- libvirt domain XML;
- root disk image and cloud-init seed lifecycle;
- k3s token, kubeconfig, node roles, labels, taints, and infrastructure add-ons;
- VM autostart, restart, destroy, and rebuild operations;
- virtiofs export definitions.
- `virtiofsd` process lifecycle.
- vhost-user socket paths and permissions.
- libvirt/QEMU filesystem device reconciliation.
- health checks for the VM-facing storage channel.
- a narrow API used by the CSI controller.

This avoids relying on hand-edited libvirt XML or TrueNAS UI VM definitions.
TrueNAS middleware can regenerate appliance-owned VM definitions during UI
operations, restarts, and upgrades. For this project, the cleaner design is to
keep Kubernetes node domains under a separate `nas-csi` ownership boundary.

### Host-Agent Control Loop

The host agent runs on TrueNAS and reconciles generated local desired state.
This example uses placeholders because concrete values come from discovery and
operator selection during `init`:

```yaml
nodes:
  - name: <generated-node-name>
    domain: <generated-libvirt-domain>
    vcpus: <derived-or-selected-vcpu-count>
    memory_mib: <derived-or-selected-memory>
    root_disk:
      pool_dataset: <selected-vm-state-dataset>
      size_gib: <derived-or-selected-size>
      image: <selected-or-discovered-cloud-image>
    network:
      bridge: <discovered-bridge>
      mac: <generated-mac-address>
    cloud_init:
      ssh_authorized_keys:
        - <operator-provided-public-key>
      k3s_role: server
exports:
  - volume_id: <generated-volume-id>
    dataset: <selected-truenas-dataset>
    source_path: /mnt/<pool>/<dataset>
    tag: <generated-virtiofs-tag>
    mode: read-write
    policy: <selected-policy>
  - volume_id: <generated-readonly-volume-id>
    dataset: <selected-truenas-dataset>
    source_path: /mnt/<pool>/<dataset>
    tag: <generated-virtiofs-tag>
    mode: read-only
    policy: <selected-policy>
```

For each export, the agent:

1. validates that the TrueNAS dataset exists and is mounted;
2. validates that the export source path is exactly under `/mnt/<pool>/...`;
3. ensures the target node VM domain exists and matches desired state;
4. ensures the k3s cluster exists and the node has the desired role;
5. starts or verifies one `virtiofsd` process per VM/export pair;
6. creates a stable socket under `/run/nas-csi/virtiofs/<node>/<volume>.sock`;
7. ensures the target VM has shared-memory backing required by virtiofs;
8. ensures the target VM has a `vhost-user-fs-pci` device using the socket and
   expected mount tag;
9. reports export health to the CSI controller and node plugin.

The preferred v1 behavior is static-at-boot attachment: all known dataset
exports for the node VM are present when the VM starts. Hotplug can be added
only after the lab proves the TrueNAS/libvirt/QEMU versions handle virtiofs
hotplug reliably with preplanned PCI topology.

### TrueNAS VM Integration Strategy

There are two integration levels:

1. Preferred: host-agent-managed node VM. TrueNAS manages storage only. The
   agent owns node VM libvirt domain XML, root disk, cloud-init seed, autostart,
   and virtiofs wiring.
2. Compatibility: adopted TrueNAS UI VM. The agent discovers the VM and tries to
   reconcile virtiofs state after TrueNAS lifecycle events.

The preferred mode is easier to make reliable because the same controller owns
the memory backing, PCI topology, virtiofs devices, root disk, and node
bootstrap. It also makes node rebuilds disposable: data lives in TrueNAS
datasets, not in the node root disk.

## Cluster Ownership

The same host agent should own the substrate k3s cluster. This keeps node VM
identity, virtiofs mount tags, CSI node identity, and Kubernetes node identity in
one desired-state model.

Cluster ownership includes:

- pinned k3s version;
- first server bootstrap;
- server and agent join configuration;
- k3s token and kubeconfig handling;
- node labels and taints;
- installation of `nas-csi` and required substrate add-ons;
- cluster status and upgrade planning.

Cluster ownership excludes user application workloads. The project should leave
applications to GitOps, Helm, or manually applied manifests after the cluster and
storage layer are healthy.

Multi-node cluster ownership is for node-level maintenance continuity. It lets
the host agent drain, rebuild, and upgrade one VM while other VMs keep running
eligible workloads. It is not a claim of physical HA because the TrueNAS host is
still the single physical failure domain.

## Why Virtiofs

Virtiofs is designed for sharing a host directory tree with a guest while
preserving local filesystem semantics more closely than older VM file sharing
mechanisms. It avoids the normal TCP/IP storage path used by NFS and SMB. That
is the only plausible way to make same-host VM access materially better for
small-file workloads while keeping the data as a TrueNAS filesystem dataset.

This is topology-specific. It only applies when the Kubernetes node VM runs on
the same physical TrueNAS/KVM host as the dataset.

## Workload Policies

The driver should not pretend one set of virtiofs options is right for all
datasets.

`repos-dev`
: Read-write git repository datasets. Use conservative coherency defaults:
  `cache=auto` for the initial benchmark, switchable to `cache=none` if SMB-side
  edits are not visible quickly enough. Disable writeback until proven safe.
  Enable flock/POSIX locks and xattrs. Raise `virtiofsd` file descriptor limits
  aggressively because package managers and repository tools touch many files.

`samples-ro`
: Read-only VST/Kontakt/library datasets. Mount read-only in Kubernetes. Use
  more aggressive read caching when benchmarked safe. Treat SMB-side content
  updates as publish events that trigger service reload or remount, not as
  random concurrent writes.

## Unsupported Topology

If the Kubernetes node VMs run on a different hypervisor or physical machine
from TrueNAS, this design cannot provide local-like access to the same dataset.
At that point every correct same-dataset design is a remote filesystem protocol:
SMB, NFS, or something equivalent.

For that topology, a custom CSI driver can make mounting and permissions cleaner,
but it cannot remove network filesystem latency from `npm install`-style
metadata-heavy workloads.

## Use Case: Git Repositories

Git repositories stay in a selected TrueNAS filesystem dataset, and TrueNAS can
expose that dataset over SMB to LAN clients.

Kubernetes remote terminal pods receive the same dataset through a CSI volume
that ultimately bind-mounts the VM's virtiofs mount.

Correctness rules:

- Treat the repository as a shared filesystem. Concurrent SMB edits and
  Kubernetes writes can conflict at the application level.
- Do not use unsafe guest-side caching for this dataset if SMB clients may edit
  files while the Kubernetes workload is active.
- Tune the ZFS dataset for small-file metadata-heavy work. This can include
  dataset `recordsize`, ACL behavior, atime behavior, and pool hardware such as
  a metadata/special vdev where appropriate.
- Expect to benchmark `git status`, package manager install, and clean builds.
  Sequential throughput alone does not prove this use case.

This keeps the data single-sourced. It does not require a separate build copy.

## Use Case: VST/Kontakt Streaming

The sample/library dataset remains a TrueNAS filesystem dataset and can stay
SMB-writable for management from LAN machines.

The Kubernetes streaming server should mount the dataset read-only through CSI.
That gives the server a stable view and prevents the pod from mutating managed
content.

Operational rules:

- Content updates should be published atomically where possible, for example
  write a new file and rename it into place.
- Large library refreshes should have an explicit rescan/reload path in the
  streaming service.
- Guest-side caching can be more aggressive for this dataset than for active git
  repositories, but cache coherency must be tested against SMB-side updates.

## CSI Surface

### Controller Service

`CreateVolume`
: Create or register a TrueNAS filesystem dataset. The driver must reject zvol
  mode for this storage class.

`DeleteVolume`
: Delete only driver-created datasets. Existing/imported datasets should default
  to retention unless the volume explicitly opts into destructive cleanup.

`ControllerPublishVolume`
: Ensure the target Kubernetes node VM is authorized and configured to receive
  the dataset through the host passthrough layer.

`ControllerUnpublishVolume`
: Remove node-specific publish state when no workloads need the dataset on that
  node. Whether the virtiofs device is removed immediately should be a policy
  choice because device churn can disrupt other pods.

`ValidateVolumeCapabilities`
: Accept filesystem volumes. Accept `MULTI_NODE_READER_ONLY` and
  `MULTI_NODE_MULTI_WRITER` only for transports and cache modes that have been
  tested for the dataset policy. Reject block volume mode.

`CreateSnapshot` and `DeleteSnapshot`
: Use TrueNAS/ZFS snapshots on the filesystem dataset.

`CreateVolume` from snapshot/source
: Clone a filesystem dataset from a snapshot when requested. This creates a new
  dataset, not a copy inside the same dataset.

### Node Service

`NodeGetInfo`
: Return the Kubernetes node identity and host VM identity used by the
  controller to reconcile virtiofs devices.

`NodeStageVolume`
: Confirm the virtiofs mount exists, mount it if needed, verify the dataset
  identity, and prepare a stable staging path.

`NodePublishVolume`
: Bind-mount the staged dataset or requested subpath into the pod target path.

`NodeUnpublishVolume`
: Remove the pod bind mount.

`NodeUnstageVolume`
: Remove the staging mount when no pod references remain.

`NodeExpandVolume`
: No filesystem grow is needed for ordinary ZFS filesystem datasets. Quota and
  reservation changes happen through the TrueNAS API.

## TrueNAS Integration

Use the TrueNAS API for normal storage and sharing operations.

Relevant API families:

- `pool.dataset.*` for filesystem dataset create, update, query, quota, and
  deletion.
- `pool.snapshot.*` for snapshot, clone, delete, and rollback operations.
- `sharing.smb.*` for SMB share discovery or optional management.
- `service.*` for SMB service readiness checks.
- `vm.*` for inventory and diagnostics only when comparing against TrueNAS UI
  VM behavior.

The project does not depend on TrueNAS exposing first-class virtiofs device
management. The host agent owns libvirt/QEMU VM lifecycle directly and presents
a narrow, auditable API to the CSI controller.

The host-side agent should use the TrueNAS API for dataset/share/snapshot
inventory. It should use libvirt/QEMU directly only for the VM transport surface.
Mixing those concerns inside the CSI controller would make failure recovery
harder.

## Node Requirements

The node DaemonSet needs privileged host access typical for CSI node plugins:

- `/var/lib/kubelet/plugins` and `/var/lib/kubelet/pods` with bidirectional
  mount propagation.
- The ability to perform bind mounts.
- A Linux guest kernel with virtiofs support.
- A stable mount table path for host datasets.

It does not need `iscsiadm`, `nvme-cli`, `mkfs`, or block device discovery for
this storage class.

## Failure Model

The driver must be idempotent and conservative.

- Existing datasets must never be deleted by accident.
- Dataset identity must be checked by TrueNAS dataset name or a stable
  driver-owned property, not only by a path string.
- Node publish should fail clearly if the virtiofs source is absent rather than
  creating an empty directory that hides the problem.
- Cache mode must be part of the dataset policy because SMB-side edits and
  guest-side caching interact directly.
- The driver must document and test lock behavior between SMB clients and
  Kubernetes workloads. Application-level concurrent writes are still a real
  risk on a shared filesystem.

## Designs To Avoid

- zvol CSI for datasets that must also be SMB-visible at the file level.
- Mounting the same zvol filesystem on TrueNAS and in Kubernetes.
- A CSI driver that hides NFS under new names while claiming to solve small-file
  performance.
- Guest-side writeback caching for datasets actively modified over SMB unless
  coherency behavior has been proven under workload.
- Treating VM snapshots as backups of virtiofs data. The authoritative snapshots
  are ZFS snapshots on TrueNAS.

## Test Plan

Unit tests:

- TrueNAS client request/response handling.
- Dataset registration and destructive-delete protection.
- Storage-class and volume-attribute validation.
- CSI publish/unpublish idempotency.

Integration tests:

- Fake TrueNAS API server with persistent dataset/share/snapshot state.
- Node mount wrappers tested with fake `mount`, `findmnt`, and bind mount state.
- Host-agent API tests for VM passthrough reconciliation.

Lab tests:

- One K3s node VM on the TrueNAS host with virtiofs dataset passthrough.
- Optional second node VM with the same dataset passthrough.
- SMB client editing files while a pod observes the same tree.
- `git status`, `git checkout`, `npm install`, and clean build benchmarks.
- Read-only VST/Kontakt streaming workload with SMB-side content refresh.
- ZFS periodic snapshot and replication while Kubernetes workloads are active.
- Node reboot while volumes are staged.
- Controller restart during publish/unpublish.

## First Milestone

The first useful milestone is a transport proof, not a full CSI driver:

1. Pick one selected filesystem dataset from discovery.
2. Export it through SMB from TrueNAS.
3. Attach the same host path to one Linux node VM with virtiofs.
4. Mount it inside the VM.
5. Run `git status`, `npm install`, and a clean build from the VM path.
6. Edit files over SMB from another LAN client and verify visibility and cache
   behavior inside the VM.
7. Snapshot the dataset with TrueNAS while the VM can still read it.
8. Repeat with the VST/Kontakt dataset mounted read-only in the VM.

Only after this passes should the behavior be wrapped in CSI.
