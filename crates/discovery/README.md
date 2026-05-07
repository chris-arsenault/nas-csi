# crates/discovery

Read-only host discovery library used by `nas-csi-host-agent`.

Responsibilities:

- discover TrueNAS version and API readiness;
- list pools, filesystem datasets, mountpoints, and SMB shares;
- discover libvirt URI and capabilities;
- discover bridges, LAN addresses, CPU, and memory;
- discover QEMU version, firmware, machine types, and CPU modes;
- discover `virtiofsd` path and version;
- discover existing `nas-csi` datasets, domains, sockets, and generated state;
- produce a `DiscoveryInventory` consumed by config generation and planning.

This crate must not mutate TrueNAS, libvirt, filesystems, or Kubernetes.

Current implementation:

- uses local `midclt call` for TrueNAS dataset and SMB inventory when available;
- falls back to `/mnt` pool scanning with warnings when middleware discovery is
  unavailable;
- records tool paths for `virsh`, QEMU, `qemu-img`, `virtiofsd`, `systemctl`,
  and `midclt`.
