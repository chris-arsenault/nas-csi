# deploy/cloud-init

Example cloud-init snippets for host-agent-managed node VMs.

Planned examples:

- base Linux node with SSH and qemu guest agent;
- k3s server bootstrap;
- k3s agent bootstrap;
- `/etc/rancher/k3s/config.yaml` rendering;
- k3s token injection from host-agent state;
- `/etc/nas-csi/node.yaml` generation;
- node VM package baseline for virtiofs and CSI node plugin support.

The host agent should generate final NoCloud seed data from typed config rather
than copying these examples verbatim.
