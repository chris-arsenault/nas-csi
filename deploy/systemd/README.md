# deploy/systemd

Systemd packaging for the TrueNAS host-agent.

Planned files:

- `nas-csi-host-agent.service`
- `nas-csi-host-agent.env`
- install script for the target TrueNAS host
- uninstall script that leaves datasets untouched

The unit should start after TrueNAS middleware, libvirt, and local filesystems
are available. It should not start Kubernetes workloads by itself.
