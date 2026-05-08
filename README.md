# nas-csi

`nas-csi` is an experimental Kubernetes CSI project for exposing ordinary
TrueNAS ZFS filesystem datasets to VM-based Kubernetes or K3s workloads.

The target topology is intentionally narrow: one TrueNAS host, IaC-managed
KVM/libvirt node VMs on that same host, a host-agent-owned k3s cluster inside
those VMs, and TrueNAS filesystem datasets that may also remain SMB shares.

## Motivation

The useful data already lives in normal TrueNAS datasets. For this project,
those datasets must keep working with:

- TrueNAS ZFS snapshots, replication, quotas, and retention tooling;
- TrueNAS SMB shares for LAN clients;
- direct file management outside Kubernetes;
- Kubernetes workloads that need better small-file and streaming behavior than
  a conventional NFS mount can provide.

That rules out treating the important datasets as opaque block volumes. Block
CSI remains useful for app-private storage, but it does not satisfy the
"same dataset, same files, still SMB-visible" requirement.

## Major Features

- TrueNAS-hosted node VM lifecycle planning through libvirt/QEMU.
- Host-agent-managed virtiofs transport for selected filesystem datasets.
- k3s bootstrap and substrate reconciliation for server and agent node VMs.
- CSI controller and node-plugin crates for the Kubernetes integration surface.
- Host-agent-owned k3s cluster reconciliation for bootstrap, join, kubeconfig,
  node readiness, labels/taints, and substrate manifests.
- Generated CSI protobuf bindings and Rust gRPC services for controller and
  node paths.
- Repo-safe intent files plus target-host discovery and local materialization.
- State-aware host reconciliation with `apply`, `skip`, and `refuse` decisions.
- Rust-generated cloud-init NoCloud seed images, with no external ISO tooling.
- Project-owned libvirt metadata hashes for stable managed-domain comparison.
- Execute safety for base image validation, root disk growth, virtiofs socket
  readiness, and explicit libvirt domain adoption.

## Current Build Slice

The repository currently implements the Rust workspace, typed config model,
read-only discovery, host config materialization, VM artifact rendering,
cloud-init seed generation, state-aware host reconciliation, cluster substrate
reconciliation, and guarded execute paths for VM/runtime and k3s substrate
changes.

```bash
cargo run -p nas-csi-host-agent -- validate-intent \
  --intent examples/intents/maintenance-basic.yaml

cargo run -p nas-csi-host-agent -- discover \
  --output .nas-csi/discovery.yaml

cargo run -p nas-csi-host-agent -- materialize \
  --intent examples/intents/maintenance-basic.yaml \
  --discovery examples/configs/discovery.sample.yaml \
  --selections examples/configs/selections.sample.yaml \
  --output .nas-csi/host.yaml

cargo run -p nas-csi-host-agent -- render \
  --config .nas-csi/host.yaml \
  --output-dir .nas-csi/rendered

cargo run -p nas-csi-host-agent -- apply \
  --config .nas-csi/host.yaml \
  --artifact-dir .nas-csi/rendered

cargo run -p nas-csi-host-agent -- status \
  --config .nas-csi/host.yaml

cargo run -p nas-csi-host-agent -- health \
  --config .nas-csi/host.yaml

cargo run -p nas-csi-host-agent -- cluster plan \
  --config .nas-csi/host.yaml \
  --artifact-dir .nas-csi/rendered \
  --manifest-root deploy
```

Generated `.nas-csi` files are host-local state and ignored by git. `apply` is
dry-run unless `--execute` is passed. Execute refuses unsafe plans, validates
base image existence/format/SHA-256 before root disk creation, grows but never
shrinks root disks, waits for managed virtiofs sockets, and refuses to manage
unmarked libvirt domains unless adoption is explicitly enabled.

`cluster apply --execute` is the follow-on substrate path. It generates the k3s
token if needed, starts the first server, retrieves and rewrites kubeconfig,
starts joining nodes, waits for readiness, reconciles labels and taints, and
applies configured substrate manifests. It does not deploy user application
workloads.

Build a TrueNAS host install directory with:

```bash
cargo run -p nas-csi-xtask -- package-host-agent
```

## Documentation

- [Documentation index](docs/README.md)
- [Design overview](docs/design/README.md)
- [Architecture](docs/design/architecture.md)
- [VM management](docs/design/vm-management.md)
- [Cluster management](docs/design/cluster-management.md)
- [Configuration](docs/design/configuration.md)
- [Discovery and onboarding](docs/design/discovery.md)
- [Component structure](docs/design/component-structure.md)
- [Project plan](docs/project/README.md)
- [Implementation plan](docs/project/implementation-plan.md)
- [Operational runbooks](docs/operations/README.md)
- [Research notes](docs/research/README.md)
- [Examples](examples/README.md)

## Development

Run the full local check:

```bash
cargo run -p nas-csi-xtask -- check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for development workflow and
[CODEX.md](CODEX.md) for project-specific instructions for Codex-style coding
agents.
