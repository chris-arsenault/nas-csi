# deploy/systemd

Systemd packaging for the TrueNAS host-agent.

Files:

- `nas-csi-host-agent.service`: oneshot reconcile unit for the TrueNAS host.
- `nas-csi-host-agent.env`: default environment file installed under
  `/etc/nas-csi`.
- `install.sh`: installs the release binary, unit, environment file, local
  directories, and packaged substrate manifests.
- `uninstall.sh`: removes only the unit and binary; it leaves config, state,
  logs, secrets, and all TrueNAS datasets untouched.

Build a package directory with:

```sh
cargo run -p nas-csi-xtask -- package-host-agent
```

Then copy `dist/host-agent` to the target host and run:

```sh
cd dist/host-agent
sudo ./install.sh
```

The package install step only installs the binary, unit, directory layout, and
packaged manifests. First host bring-up is performed by the installed binary:

```sh
sudo nas-csi-host-agent host-install \
  --intent /etc/nas-csi/intent.yaml \
  --selections /etc/nas-csi/selections.yaml
```

Add `--execute` after reviewing the dry-run output. The execute path starts the
managed domains by default and verifies root disks, seed images, libvirt domain
ownership, virtiofsd units, sockets, qemu guest agent response, health output,
idempotence, and dataset observations.

After rebooting TrueNAS, rerun the installer in verification-only mode:

```sh
sudo nas-csi-host-agent host-install \
  --post-reboot-check \
  --config /etc/nas-csi/host.yaml
```

After host bring-up succeeds, run cluster substrate bring-up:

```sh
sudo nas-csi-host-agent cluster install \
  --config /etc/nas-csi/host.yaml
```

Add `--execute` after reviewing the cluster dry-run output. The cluster
installer owns token generation, first-server startup, kubeconfig retrieval,
join-node startup, API/node readiness, labels/taints, substrate manifest apply,
and idempotence checks. To validate VM maintenance, add `--reboot-node NAME`.
After rebooting TrueNAS, run:

```sh
sudo nas-csi-host-agent cluster install \
  --config /etc/nas-csi/host.yaml \
  --post-reboot-check
```

After the k3s substrate is healthy, run static existing-dataset CSI bring-up:

```sh
sudo nas-csi-host-agent csi install \
  --config /etc/nas-csi/host.yaml
```

Add `--execute` after reviewing the dry-run output. The CSI installer writes
VM-local node runtime config, applies the pinned `nas-csi` manifest, generates
static PV/PVCs for configured exports, and runs smoke checks for virtiofs
staging, pod publishing, restarts, missing exports, read-only mounts, and
host/pod directory visibility. It does not install application workloads.

The unit starts after local filesystems, networking, TrueNAS middleware, and
libvirt service names. It runs `nas-csi-host-agent apply --execute` using paths
from `/etc/nas-csi/host-agent.env`. It does not install application workloads.

## Directory Layout

`/etc/nas-csi` mode `0750`
: Host-local configuration. Contains `host.yaml` and `host-agent.env`.

`/etc/nas-csi/secrets` mode `0700`
: Secret file directory. Expected files include `truenas-api-key` and
  `k3s-token`. Secret files should use mode `0600`.

`/var/lib/nas-csi` mode `0750`
: Host-agent state. The default rendered artifact directory is
  `/var/lib/nas-csi/rendered`.

`/var/log/nas-csi` mode `0750`
: Host-agent log directory for file-based deployments. The current unit writes
  structured logs to journald; file logging can be added without changing the
  layout.

`/run/nas-csi` mode `0750`
: Runtime files and virtiofs sockets. The unit also sets
  `RuntimeDirectory=nas-csi` so systemd recreates it on boot.

`/usr/local/share/nas-csi/deploy` mode `0755`
: Installed Kubernetes substrate manifests for `cluster apply --execute`.

`/usr/local/sbin/nas-csi-host-agent` mode `0755`
: Installed host-agent binary.
