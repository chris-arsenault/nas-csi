# Architecture

## Purpose

`nas-csi` exposes ordinary TrueNAS ZFS filesystem datasets to a VM-hosted k3s
cluster without turning those datasets into block volumes or separate copies.

The core requirement is that the same file tree remains:

- a normal TrueNAS filesystem dataset;
- managed by TrueNAS snapshots, replication, quotas, and retention tooling;
- optionally SMB-visible to LAN clients;
- mountable by Kubernetes workloads with better same-host behavior than NFS.

The design only targets node VMs running on the same TrueNAS/KVM host as the
datasets. If the Kubernetes nodes move to another physical host, the correct
transport becomes a network filesystem again. See
[Deployment Scope](scope.md) for first-deploy rules and non-goals.

## Ownership

TrueNAS owns authoritative storage:

- pools and filesystem datasets;
- SMB shares;
- ZFS snapshots, replication, quotas, and retention;
- host boot and appliance services.

`nas-csi-host-agent` owns the local compute and transport substrate:

- generated host desired state;
- node VM libvirt domains;
- root disk overlays and cloud-init seeds;
- `virtiofsd` systemd units and sockets;
- k3s bootstrap, kubeconfig, node readiness, labels, and taints;
- Kubernetes substrate manifests for `nas-csi` and required add-ons.

Kubernetes owns runtime orchestration:

- pod scheduling;
- PV/PVC binding;
- CSI sidecars and plugin registration;
- pod-facing bind mounts inside node VMs.

Application deployment is deliberately outside this repository. User workloads
belong in a separate GitOps, Helm, or manual deployment flow after the substrate
is healthy.

## Data Path

The runtime data path is intentionally simple:

1. TrueNAS mounts a selected filesystem dataset at its normal host path.
2. TrueNAS may export that same path over SMB.
3. The host agent starts one `virtiofsd` service per node/export pair.
4. The node VM has a libvirt virtiofs filesystem device for that export.
5. Cloud-init configures the guest virtiofs mount under
   `/var/lib/nas-csi/virtiofs`.
6. `csi install` refreshes `/etc/nas-csi/node.yaml` in each VM from host-local
   config and applies static PV/PVC manifests for the configured exports.
7. The CSI node plugin validates the virtiofs mount and bind-mounts it into
   kubelet staging and pod target paths.

The authoritative files never move into Kubernetes-owned block storage. VM root
disks are disposable substrate; application data remains in TrueNAS datasets.

## Control Path

The host-side control path starts from local desired state:

1. `discover` builds read-only host inventory.
2. `init` creates a repo-safe draft of required selections.
3. `materialize` combines intent, discovery, and local selections into
   `HostConfig`.
4. `render` writes artifacts for review.
5. `apply` or `host-install` reconciles VM/runtime substrate.
6. `cluster apply` or `cluster install` reconciles k3s and Kubernetes
   substrate.
7. `csi install` installs static existing-dataset CSI state and runs smoke
   verification.
8. `workload validate` runs the real repository and read-only streaming probes
   against selected existing datasets.
9. `status` and `health` report actual host-side state.

CSI control is split by responsibility:

- the controller service handles CSI Identity and Controller RPCs;
- the node service handles CSI Identity and Node RPCs;
- generated CSI protobuf bindings live in `crates/csi-proto`;
- TrueNAS dataset, SMB, and snapshot operations are reached through the
  TrueNAS client/backend boundary;
- VM and virtiofs transport changes stay on the TrueNAS host-agent side.

The controller loads `/etc/nas-csi/controller.yaml` at startup. Its durable
metadata model is a local JSON state file, mounted at
`/var/lib/nas-csi/controller/state.json` by the static manifest. That file
preserves controller-owned volume, snapshot, and publish identity across
controller restarts. The TrueNAS backend uses JSON-RPC over WebSocket with API
key authentication, connect/request timeouts, and bounded reconnect attempts.

Dynamic dataset creation remains disabled by default. When enabled, a PVC still
needs an explicit dynamic mode parameter. Dataset deletion requires both
controller configuration and a per-volume delete opt-in.

Static existing datasets are the first supported deployment target. Dynamic
dataset create/delete is a controller capability for later opt-in use, not the
default storage path for the initial deployment.

## Reconciliation Model

Host reconciliation is state-aware and conservative.

VM/runtime `apply` inspects actual files, tools, systemd units, qemu images, and
libvirt domains before deciding whether to apply, skip, or refuse a change.
Unsafe mutations, such as replacing an existing root disk or redefining a
running VM without an explicit flag, become refusals.

Cluster reconciliation builds on the VM/runtime layer. It generates the k3s
token if missing, starts the first server, waits for guest readiness, retrieves
and rewrites kubeconfig, starts join nodes, waits for Kubernetes node readiness,
reconciles labels and taints, and applies substrate manifests with local hash
markers.

CSI node operations are fail-closed. If the expected runtime config, virtiofs
tag, mount type, or staging mount is absent, the plugin returns an error instead
of creating an empty directory that would hide the broken transport.

## Dataset Policies

Policies describe the expected behavior of shared datasets. They are not merely
Kubernetes access modes.

`repos-dev`
: Read-write repository datasets. These need conservative cache settings and
  explicit SMB coherency testing because LAN clients and Kubernetes workloads
  may both touch the same tree.

`samples-ro`
: Read-only sample/library datasets for streaming workloads. Kubernetes mounts
  should be read-only, while SMB remains the management path for content
  updates.

The code owns policy enforcement at the CSI and node layers. The docs only
describe the intended boundary and operational consequences.

## Kubernetes Surface

The installable substrate manifest provides:

- CSI controller Deployment and standard controller sidecars;
- CSI node DaemonSet and node-driver-registrar;
- RBAC;
- `CSIDriver`;
- StorageClasses for existing datasets and retained dynamic datasets;
- example PV/PVC manifests kept outside the applied substrate path.

The cluster installer applies these manifests only as cluster substrate. The CSI
installer separately generates static PV/PVC objects for configured existing
datasets, with `Retain` reclaim policy and node affinity for the VMs that expose
each export. Neither installer deploys application workloads.

`workload validate` is the explicit exception for lab validation. It deploys
temporary validation pods, exercises the selected repository and content
datasets, records host/guest/pod coherency and `virtiofsd` behavior, then
removes the pods on success. It is not an application deployment mechanism.

## Failure Model

Important safety rules:

- Existing TrueNAS datasets are authoritative and must not be deleted by normal
  VM or cluster rebuilds.
- Dynamic dataset deletion requires explicit controller configuration.
- Root disks can grow but are not destructively replaced by `apply`.
- Libvirt domains must carry `nas-csi` metadata before adoption, unless the
  operator explicitly opts in.
- VM snapshots are not backups of virtiofs-shared data. TrueNAS ZFS snapshots
  on the datasets are the authoritative data-protection layer.
- Multi-node k3s is for planned maintenance continuity, not physical HA. The
  TrueNAS host remains the single physical failure domain.

## Lab Priorities

The next validation work should prove behavior on the target host rather than
expand design text:

- boot host-agent-owned VMs on TrueNAS SCALE;
- run `workload validate --execute` for repository performance, SMB-visible
  coherency, read-only streaming, and `virtiofsd` restart behavior;
- exercise `cluster apply --execute` against real k3s nodes;
- run CSI static-PV mount tests with the node plugin;
- validate snapshots and replication while pods are reading or writing.
