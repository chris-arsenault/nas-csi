# Research Notes

These notes capture the design facts that matter for this project. They are not
general NAS advice.

Detailed notes:

- [TrueNAS virtiofs integration](truenas-virtiofs.md)
- [IaC node VMs on libvirt](libvirt-iac-vms.md)
- [TrueNAS VM UI integration](truenas-vm-ui-integration.md)
- [k3s cluster lifecycle](k3s-lifecycle.md)
- [virtiofsd fork strategy](virtiofsd-fork-strategy.md)
- [Rust CSI implementation](rust-csi.md)
- [SMB and virtiofs coherency](smb-virtiofs-coherency.md)

## Source-Backed Findings

TrueNAS VM access model:

- TrueNAS documentation says VMs do not directly communicate with host NAS
  storage by default and describes bridge/network access for VM access to
  storage.
- TrueNAS VM creation centers VM disks around zvols.
- Current API documentation exposes `vm.query` and `vm.device.query`; the
  documented VM device surface does not show a first-class virtiofs filesystem
  device.
- TrueNAS API v26 has `command_line_args` for VMs, but this still needs lab
  verification before relying on it for virtiofs because virtiofs also requires
  shared-memory backing.

TrueNAS custom libvirt state:

- Community reports show hand-edited libvirt XML can be removed after TrueNAS UI
  VM restart because middleware regenerates VM config.
- That makes manual XML edits a bad source of truth.
- The host agent must reconcile state instead.

Virtiofs:

- Virtiofs is built for host/guest directory sharing and is specifically meant
  to avoid network filesystem overhead for colocated VMs.
- Linux guest support has existed since Linux 5.4.
- libvirt virtiofs requires shared memory backing.
- libvirt can connect a virtiofs device to an externally launched `virtiofsd`
  socket. That is the right hook for the host agent.
- Older virtiofsd versions have migration/save/snapshot limitations; VM
  snapshots are not backups of the shared dataset.
- The Rust `virtiofsd-rs` daemon is the active development target and a viable
  fork point.

Known virtiofs risks for this project:

- File descriptor pressure on large file trees. Red Hat documents `rsync` and
  `du` cases that can hit `too many open files`; a virtiofsd issue also reports
  high FD counts with ZFS-backed datasets and notes cache tuning as mitigation.
- Cache mode is a correctness decision when SMB clients and VM workloads touch
  the same tree.
- Hotplug requires careful libvirt/QEMU support and planned PCI topology. Static
  attachment at VM boot should be the v1 path.

Rust CSI:

- The `k8s-csi` Rust crate exists and generates CSI types/services with tonic,
  but its latest release is old and based on CSI v1.3.0.
- The safer project path is to generate bindings from the upstream CSI proto in
  our workspace with current `tonic` and `prost`.

## Sources

- TrueNAS VM management docs:
  <https://www.truenas.com/docs/scale/virtualmachines/managingvms/>
- TrueNAS JSON-RPC API:
  <https://api.truenas.com/v27.0/jsonrpc.html>
- TrueNAS VM API:
  <https://api.truenas.com/v26.04.0/api_methods_vm.create.html>
- TrueNAS VM device API:
  <https://api.truenas.com/v25.04/api_events_vm.device.query.html>
- TrueNAS SMB docs:
  <https://www.truenas.com/docs/scale/shares/smb/>
- TrueNAS snapshots docs:
  <https://www.truenas.com/docs/scale/datasets/snapshots/>
- TrueNAS community filesystem passthrough request:
  <https://www.truenas.com/community/threads/scale-feature-request-file-system-passthrough.106673/>
- TrueNAS community custom libvirt XML persistence report:
  <https://www.truenas.com/community/threads/truenas-scale-removing-custom-libvirt-options.90553/>
- Virtiofs overview:
  <https://virtio-fs.gitlab.io/>
- Virtiofs design:
  <https://virtio-fs.gitlab.io/design.html>
- libvirt virtiofs docs:
  <https://libvirt.gitlab.io/libvirt/kbase/virtiofs.html>
- QEMU virtiofsd docs:
  <https://virtio-fs.gitlab.io/qemu/tools/virtiofsd.html>
- libvirt PCI hotplug docs:
  <https://www.libvirt.org/pci-hotplug.html>
- Rust virtiofsd project:
  <https://gitlab.com/virtio-fs/virtiofsd>
- Virtiofs ZFS FD issue:
  <https://gitlab.com/virtio-fs/virtiofsd/-/work_items/121>
- Red Hat virtiofs FD issue note:
  <https://docs.redhat.com/en/documentation/red_hat_enterprise_linux/10/html/10.1_release_notes/known-issues>
- Kubernetes CSI developer docs:
  <https://kubernetes-csi.github.io/docs/>
- Kubernetes CSI deployment docs:
  <https://kubernetes-csi.github.io/docs/deploying.html>
- `k8s-csi` Rust crate:
  <https://docs.rs/crate/k8s-csi/latest>
- Upstream CSI spec:
  <https://github.com/container-storage-interface/spec>
