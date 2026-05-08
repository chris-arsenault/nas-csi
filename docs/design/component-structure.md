# Component Structure

## Runtime Boundaries

```text
TrueNAS host
  nas-csi-host-agent
    -> discovery
    -> VM/runtime reconciliation
    -> k3s cluster reconciliation
    -> static CSI existing-dataset install
    -> real workload validation
    -> virtiofsd systemd services
    -> local package/install assets

Kubernetes control plane
  CSI sidecars
    -> nas-csi-controller

Kubernetes node VM
  virtiofs guest mounts
  kubelet
    -> nas-csi-node plugin
```

The process boundary is deliberate. TrueNAS storage operations, VM transport
operations, and pod mount operations fail differently and should stay separable.

## Crates

`nas-csi-types`
: Shared config and intent types. This crate defines the durable model used by
  discovery, materialization, rendering, and node runtime config.

`nas-csi-discovery`
: Read-only host inventory. It discovers TrueNAS facts, datasets, SMB shares,
  host tools, networking, libvirt, and existing project state. It does not
  mutate the host.

`nas-csi-vm-manager`
: VM and host runtime planning. It renders libvirt XML, cloud-init, k3s config
  artifacts, NoCloud seed images, virtiofsd units, and state-aware host
  reconcile operations.

`nas-csi-cluster-manager`
: k3s and Kubernetes substrate planning. It models token and kubeconfig state,
  first-server bootstrap, join-node startup, API and node readiness, labels,
  taints, add-on manifests, and `nas-csi` manifest reconciliation.

`nas-csi-host-agent`
: TrueNAS-side CLI/daemon entrypoint. It wires discovery, materialization,
  rendering, host apply/status/health, cluster plan/apply/status, command
  execution, guarded writes, static existing-dataset CSI install, real workload
  validation, and packaging assumptions.

`nas-csi-proto`
: Generated CSI protobuf and gRPC bindings. Generated Rust is build output and
  is not committed.

`nas-csi-driver`
: CSI controller and identity service. It handles existing dataset volumes,
  optional dynamic dataset creation hooks, delete safety, controller publish
  state, capability validation, volume listing, capacity, SMB metadata,
  snapshots, and retention/replication metadata.

`nas-csi-node-plugin`
: CSI node and identity service. It loads `/etc/nas-csi/node.yaml`, validates
  virtiofs mounts through mountinfo, stages volumes, bind-mounts into pod
  targets, and performs idempotent unpublish/unstage operations.

`nas-csi-truenas-client`
: TrueNAS JSON-RPC request/response primitives and typed method wrappers for
  the API surface this project uses. It includes a blocking WebSocket transport
  with API-key authentication, connect/request timeouts, and bounded reconnect
  attempts.

`nas-csi-xtask`
: Developer automation for workspace checks and host-agent packaging.

## Deployment Assets

`deploy/systemd`
: Host-agent install assets, environment defaults, and systemd unit.

`deploy/kubernetes/nas-csi`
: Static Kubernetes substrate manifest for the CSI controller, node plugin,
  sidecars, RBAC, `CSIDriver`, StorageClasses, and examples.

`deploy/addons`
: Optional substrate add-ons such as metrics-server. These are cluster
  infrastructure only, not applications.

`deploy/helm/nas-csi`
: Reserved for a future chart wrapper. The current installable path is the
  static manifest under `deploy/kubernetes/nas-csi`.

## Current Gaps

The component boundaries above are implemented enough to compile and test
locally. The remaining gaps are integration hardening:

- authenticated host-agent RPC for controller-to-host export reconciliation;
- TrueNAS lab validation for VM lifecycle, guest agent operations, virtiofs
  behavior, the `csi install --execute` smoke checks, and
  `workload validate --execute` report review.
