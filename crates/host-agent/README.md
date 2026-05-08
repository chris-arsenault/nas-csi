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
- Expose status and health output for operators and automation.

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
cargo run -p nas-csi-host-agent -- status \
  --config .nas-csi/host.yaml
cargo run -p nas-csi-host-agent -- health \
  --config .nas-csi/host.yaml
cargo run -p nas-csi-host-agent -- health \
  --config .nas-csi/host.yaml \
  --json
cargo run -p nas-csi-host-agent -- host-install \
  --intent examples/intents/maintenance-basic.yaml \
  --selections examples/configs/selections.sample.yaml
cargo run -p nas-csi-host-agent -- cluster plan \
  --config .nas-csi/host.yaml \
  --artifact-dir .nas-csi/rendered \
  --manifest-root deploy
cargo run -p nas-csi-host-agent -- cluster install \
  --config .nas-csi/host.yaml \
  --manifest-root deploy
cargo run -p nas-csi-host-agent -- csi install \
  --config .nas-csi/host.yaml \
  --manifest-root deploy
cargo run -p nas-csi-host-agent -- workload validate \
  --config .nas-csi/host.yaml
```

`materialize`, `render`, and default `apply` are non-mutating. `apply` renders
the desired host plan, inspects actual local state, and prints a reconcile plan
with `apply`, `skip`, or `refuse` decisions for artifact writes, root disk
overlays, cloud-init seed images, virtiofsd systemd units, and libvirt domain
definitions. The only mutating path is `apply --execute`; execution refuses to
run if any step is unsafe or blocked by missing host state.

Execution is routed through a command runner abstraction that accepts only
`program + argv` command specs. `--execute` refuses all changes if any reconcile
step refused, never replaces an existing root disk, and refuses running-domain
XML changes unless `--allow-running-domain-redefine` is passed. VM start remains
out of the default apply path. Existing libvirt domains without `nas-csi`
metadata are refused unless `--allow-domain-adoption` is passed, and virtiofsd
start/restart operations wait for the expected Unix socket before continuing.

Generated `HostConfig` carries discovered host tool paths under `hostTools`.
Those paths feed render and apply, so systemd units use the discovered
`virtiofsd` binary and host commands use the discovered `qemu-img`, `virsh`, and
`systemctl` binaries.

Guarded execution:

```sh
cargo run -p nas-csi-host-agent -- apply \
  --config .nas-csi/host.yaml \
  --artifact-dir .nas-csi/rendered \
  --execute
```

Additional explicit escape hatches exist for narrow VM operations:

- `--allow-running-domain-redefine` permits redefining a running managed domain.
- `--allow-domain-adoption` permits adopting an existing stopped libvirt domain
  that lacks `nas-csi` metadata.

Both options are intentionally absent from the normal apply example.

First-host install workflow:

```sh
nas-csi-host-agent host-install \
  --intent /etc/nas-csi/intent.yaml \
  --selections /etc/nas-csi/selections.yaml
```

`host-install` is the installer-style bring-up path for the TrueNAS host. It
runs read-only discovery, writes `/etc/nas-csi/discovery.yaml`, materializes and
writes `/etc/nas-csi/host.yaml`, prints the host reconcile dry-run, and then
stops unless `--execute` is passed. With `--execute`, it runs the guarded host
apply path, starts the managed domains by default, waits for qemu guest agent
response, prints status and health, verifies root disks, seed images, libvirt
ownership/autostart, virtiofsd units, sockets, and dataset observations, and
then verifies that a second reconcile would be idempotent.

After rebooting TrueNAS, run:

```sh
nas-csi-host-agent host-install \
  --post-reboot-check \
  --config /etc/nas-csi/host.yaml
```

That mode does not rerun discovery or rewrite config. It inspects the existing
host state, prints status and health, verifies persistent root disk, seed image,
libvirt, virtiofsd, socket, qemu guest agent, and idempotence state, and fails
closed if the reboot changed the expected substrate.

Use `--no-start-domains` only when intentionally preparing definitions without
booting node VMs. In that mode the qemu guest agent check is skipped.

The `health` command reports required host tools, managed virtiofsd systemd
units, libvirt domains, virtiofs sockets, and dataset mountpoints. Human output
is intended for operators; `--json` is intended for automation and runbooks.

The `cluster` subcommands reconcile the k3s substrate after VM/runtime apply:
token generation, first-server startup, kubeconfig retrieval, join-node startup,
API/node readiness, label/taint reconciliation, and substrate manifest apply.

Installer-style cluster bring-up:

```sh
nas-csi-host-agent cluster install \
  --config /etc/nas-csi/host.yaml
```

`cluster install` loads only substrate manifests from the installed deploy root,
prints the reconcile plan, and stops unless `--execute` is passed. With
`--execute`, it generates or validates the k3s token, starts the first server,
waits for the API, retrieves and rewrites kubeconfig, starts join nodes in
order, waits for Kubernetes node readiness, reconciles labels and taints,
applies only metrics-server and `nas-csi`, verifies token and kubeconfig
permissions, checks the configured API endpoint, verifies manifest markers, and
fails if a second reconcile would still apply changes.

To exercise the maintenance reboot path for one VM node:

```sh
nas-csi-host-agent cluster install \
  --config /etc/nas-csi/host.yaml \
  --execute \
  --reboot-node agent-1
```

After rebooting TrueNAS, run:

```sh
nas-csi-host-agent cluster install \
  --config /etc/nas-csi/host.yaml \
  --post-reboot-check
```

That mode inspects the cluster and verifies token, kubeconfig, API readiness,
node readiness, labels, taints, substrate manifests, and idempotence without
reapplying manifests.

Static existing-dataset CSI install:

```sh
nas-csi-host-agent csi install \
  --config /etc/nas-csi/host.yaml
```

`csi install` is the storage bring-up path for existing TrueNAS datasets. It
loads the installed `nas-csi` manifest, verifies the lab image tags, renders
per-node `/etc/nas-csi/node.yaml` from `HostConfig`, and stops unless
`--execute` is passed.

With `--execute`, it writes node runtime config into each VM through the qemu
guest agent, applies the `nas-csi` Kubernetes manifest, renders and applies
static PV/PVC objects for every configured export, waits for the controller and
node plugin rollouts, verifies guest virtiofs mounts, creates smoke pods pinned
to nodes that expose each export, verifies pod bind mounts, exercises pod and
node-plugin restarts, confirms missing exports fail closed, checks read-only
mount flags for read-only exports, and compares host dataset top-level entries
with the pod view.

Generated static manifests are written under
`/var/lib/nas-csi/rendered/csi` by default. PVs use `Retain` reclaim policy,
`nas-csi-existing` storage class, and node affinity matching the VM nodes that
actually have each export. The installer reads dataset directories for
verification but does not create, delete, or rewrite files inside exported
datasets.

Real workload validation:

```sh
nas-csi-host-agent workload validate \
  --config /etc/nas-csi/host.yaml
```

`workload validate` selects the first read-write export as the repository
dataset and the first read-only export as the content dataset unless
`--repo-export` or `--content-export` is provided. The default mode renders the
validation pod manifest and report scaffold under
`/var/lib/nas-csi/rendered/workload-validation` without touching datasets or the
cluster.

With `--execute`, it applies one long-running repo pod and one read-only content
pod, runs repository checks from the repo pod, runs a read-only streaming probe
from the content pod, writes temporary sentinel files under
`.nas-csi-validation` in the selected TrueNAS dataset paths, verifies the same
sentinels from VM guest mounts and pods, captures `virtiofsd` systemd state,
domain cache policy, guest mountinfo, and restart behavior, then writes a report.
The sentinel files are removed before the command exits. Validation pods are
deleted on success unless `--keep-pods` is passed.

The default content pod runs `httpd -f -p 8080 -h /content` from BusyBox. Use
`--content-image` and `--content-command` to run the actual VST/Kontakt
streaming server image while preserving the same read-only dataset mount and
reporting flow.
