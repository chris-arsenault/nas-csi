# Design

These documents describe the target architecture and ownership boundaries.

- [Architecture](architecture.md): storage model, data path, control path, CSI
  surface, and correctness boundaries.
- [VM management](vm-management.md): IaC-owned node VM lifecycle, images,
  cloud-init, libvirt domains, and reconciliation.
- [Cluster management](cluster-management.md): k3s bootstrap, node join,
  kubeconfig, add-ons, and ownership boundaries.
- [Configuration](configuration.md): desired-state model, local materialization,
  profiles, and repo boundaries.
- [Discovery and onboarding](discovery.md): target-host discovery and local
  config generation.
- [Component structure](component-structure.md): process boundaries and crate
  responsibilities.
