# Examples

Example intent files for `nas-csi-host-agent`.

Files:

- [maintenance-basic.yaml](intents/maintenance-basic.yaml): one k3s server and
  two agent VMs for rolling agent maintenance.
- [maintenance-control-plane.yaml](intents/maintenance-control-plane.yaml):
  three k3s server VMs for control-plane continuity during VM maintenance.
- [discovery.sample.yaml](configs/discovery.sample.yaml): fictional discovered
  host inventory for materialization tests.
- [selections.sample.yaml](configs/selections.sample.yaml): fictional local
  host choices used with an intent and discovery snapshot.
- [host.sample.yaml](configs/host.sample.yaml): fictional concrete `HostConfig`
  used to exercise artifact rendering.

These examples intentionally omit host-specific facts and application workloads.
The setup flow discovers TrueNAS, libvirt, network, storage, image, and dataset
details on the target host, then writes a local desired-state file outside this
repo.

The sample image checksum is a placeholder fixture. A real local selection file
must use the SHA-256 of the selected base cloud image before `apply --execute`
will create root disk overlays.
