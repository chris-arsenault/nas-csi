# crates/host-agent

TrueNAS-side daemon that owns the VM transport surface.

Responsibilities:

- Read desired node/export config.
- Run read-only discovery and generate local desired state from intent.
- Validate TrueNAS datasets and source paths.
- Bootstrap and reconcile the k3s cluster.
- Start and supervise pinned `virtiofsd-rs` processes.
- Create and clean `/run/nas-csi` sockets.
- Reconcile libvirt/QEMU filesystem devices for node VMs.
- Provide a gRPC API for the CSI controller.
- Expose health and metrics.

The host agent is intentionally specific to this deployment. It does not need to
support arbitrary hypervisors, remote nodes, or generic NAS protocols.

First implementation target:

- Host-agent-owned node VMs defined in config.
- Host-agent-owned k3s cluster bootstrap defined in config.
- Static-at-boot virtiofs attachment.
- Manual or planned VM restart required when shared-memory backing or filesystem
  devices need to change.
- Destructive node rebuilds that leave TrueNAS datasets untouched.

Current CLI surface:

```sh
cargo run -p nas-csi-host-agent -- validate-intent \
  --intent examples/intents/maintenance-basic.yaml
cargo run -p nas-csi-host-agent -- discover \
  --output .nas-csi/discovery.yaml
cargo run -p nas-csi-host-agent -- init \
  --intent examples/intents/maintenance-basic.yaml \
  --discovery .nas-csi/discovery.yaml \
  --output .nas-csi/host-draft.yaml
cargo run -p nas-csi-host-agent -- plan \
  --config .nas-csi/host-draft.yaml
cargo run -p nas-csi-host-agent -- materialize \
  --intent examples/intents/maintenance-basic.yaml \
  --discovery examples/configs/discovery.sample.yaml \
  --selections examples/configs/selections.sample.yaml \
  --output .nas-csi/host.yaml
cargo run -p nas-csi-host-agent -- plan \
  --config .nas-csi/host.yaml
cargo run -p nas-csi-host-agent -- render \
  --config .nas-csi/host.yaml \
  --output-dir .nas-csi/rendered
cargo run -p nas-csi-host-agent -- apply \
  --config .nas-csi/host.yaml \
  --artifact-dir .nas-csi/rendered
```

`materialize`, `render`, and default `apply` are non-mutating. `apply` renders
the desired host plan, inspects actual local state, and prints a reconcile plan
with `apply`, `skip`, or `refuse` decisions for artifact writes, root disk
overlays, cloud-init seed images, virtiofsd systemd units, and libvirt domain
definitions. The only mutating path is `apply --execute`; execution refuses to
run if any step is unsafe or blocked by missing host state.

Generated `HostConfig` carries discovered host tool paths under `hostTools`.
Those paths feed render and apply, so systemd units use the discovered
`virtiofsd` binary and host commands use the discovered `qemu-img`, `virsh`, and
`systemctl` binaries.
