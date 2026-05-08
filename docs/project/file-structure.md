# File Structure

The repository is a Rust workspace with deployment assets and research kept
beside the code.

```text
.
├── Cargo.toml
├── CHANGELOG.md
├── CODEX.md
├── CONTRIBUTING.md
├── LICENSE
├── README.md
├── crates
│   ├── csi-driver
│   │   └── README.md
│   ├── csi-proto
│   │   └── README.md
│   ├── cluster-manager
│   │   └── README.md
│   ├── discovery
│   │   └── README.md
│   ├── host-agent
│   │   └── README.md
│   ├── node-plugin
│   │   └── README.md
│   ├── truenas-client
│   │   └── README.md
│   ├── types
│   │   └── README.md
│   ├── vm-manager
│   │   └── README.md
│   └── xtask
│       └── README.md
├── deploy
│   ├── addons
│   │   └── README.md
│   ├── cloud-init
│   │   └── README.md
│   ├── helm
│   │   └── nas-csi
│   │       └── README.md
│   ├── kubernetes
│   │   └── nas-csi
│   │       └── README.md
│   └── systemd
│       └── README.md
├── docs
│   ├── README.md
│   ├── design
│   │   ├── README.md
│   │   ├── architecture.md
│   │   ├── cluster-management.md
│   │   ├── component-structure.md
│   │   ├── configuration.md
│   │   ├── discovery.md
│   │   └── vm-management.md
│   ├── lab
│   │   └── README.md
│   ├── operations
│   │   ├── README.md
│   │   └── runbooks
│   │       ├── README.md
│   │       ├── cluster-rebuild.md
│   │       └── node-maintenance.md
│   ├── project
│   │   ├── README.md
│   │   ├── file-structure.md
│   │   └── implementation-plan.md
│   └── research
│       └── README.md
├── examples
│   ├── README.md
│   ├── configs
│   │   ├── discovery.sample.yaml
│   │   ├── host.sample.yaml
│   │   └── selections.sample.yaml
│   └── intents
│       ├── maintenance-basic.yaml
│       └── maintenance-control-plane.yaml
├── hack
│   └── README.md
└── third_party
    └── README.md
```

## Crates

`crates/types`
: Shared domain models, volume policies, IDs, config structs, and error types.

`crates/cluster-manager`
: k3s desired state, bootstrap, node join, kubeconfig, add-on reconciliation, and
  upgrade planning library used by the host agent.

`crates/discovery`
: Read-only host inventory library. It discovers TrueNAS, libvirt, network,
  image, virtiofsd, and existing project state before local config generation.

`crates/truenas-client`
: TrueNAS JSON-RPC WebSocket client and typed wrappers for the small API surface
  the project needs.

`crates/host-agent`
: TrueNAS-side daemon. Owns node VM lifecycle, virtiofsd process supervision,
  and VM transport reconciliation.

`crates/vm-manager`
: Libvirt domain, root disk, cloud-init, and node lifecycle library used by the
  host agent.

`crates/csi-driver`
: CSI controller service and shared CSI server bootstrap.

`crates/csi-proto`
: Generated CSI protobuf and gRPC bindings used by the controller and node
  plugin crates.

`crates/node-plugin`
: CSI node service and Linux mount operations inside the Kubernetes node VM.

`crates/xtask`
: Developer automation for workspace checks, host-agent packaging, and future
  release/lab tasks.

## Docs

`docs/design`
: Architecture and ownership design.

`docs/project`
: Repository structure, implementation plan, and project planning notes.

`docs/operations`
: Operator runbooks and procedural documentation.

`docs/research`
: Source-backed research notes.

## Deploy

`deploy/addons`
: Optional substrate add-on manifests or Helm values owned by the cluster
  manager. User applications do not live here.

`deploy/kubernetes/nas-csi`
: Static Kubernetes substrate manifests for controller Deployment, node
  DaemonSet, RBAC, CSIDriver object, StorageClasses, and examples.

`deploy/helm/nas-csi`
: Reserved for a future Helm chart wrapper. The static manifest is the current
  installable path.

`deploy/cloud-init`
: Example cloud-init snippets for node bootstrap.

`deploy/systemd`
: TrueNAS host-agent unit, environment file, and install notes.

## Examples

`examples/intents`
: Schematic intent examples for maintenance-oriented k3s clusters. They contain
  profiles and counts, not host-specific generated config.

`examples/configs`
: Fictional fixtures for discovery, local selections, and concrete host config.
  They are for tests and documentation only.

## Third Party

`third_party/virtiofsd-rs`
: Optional git submodule or vendored patch area for the Rust virtiofs daemon.
  Keep it empty until the lab proves an upstream package is insufficient.
