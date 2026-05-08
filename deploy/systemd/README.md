# deploy/systemd

Systemd packaging for the TrueNAS host-agent.

Files:

- `nas-csi-host-agent.service`: oneshot reconcile unit for the TrueNAS host.
- `nas-csi-host-agent.env`: default environment file installed under
  `/etc/nas-csi`.
- `install.sh`: installs the release binary, unit, environment file, and local
  directories.
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

`/usr/local/sbin/nas-csi-host-agent` mode `0755`
: Installed host-agent binary.
