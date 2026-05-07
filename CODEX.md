# Codex Project Notes

These notes are for coding agents working in this repository.

## Core Constraints

- The same TrueNAS filesystem dataset must remain a normal ZFS dataset.
- The same dataset may also be SMB-visible.
- Do not suggest or implement a design that splits SMB data and Kubernetes data
  into separate authoritative copies.
- Do not turn shared datasets into opaque zvol/iSCSI/NVMe block volumes.
- Host-specific facts must be discovered or placed in local generated config,
  not committed.

## Engineering Direction

- The target runtime is one TrueNAS host with local KVM/libvirt node VMs.
- `nas-csi-host-agent` owns VM definitions, cloud-init seed data, root disks,
  virtiofsd units, and cluster bootstrap.
- TrueNAS owns ZFS datasets, SMB shares, snapshots, replication, quotas, and
  retention.
- Kubernetes owns workloads, CSI objects, and in-guest mount lifecycle.

## Working Rules

- Use `rg` for repository search.
- Keep changes scoped and testable.
- Prefer typed Rust planning APIs over shell string construction.
- Preserve safety-first reconciliation: a questionable host-side change should
  become `refuse`, not best-effort mutation.
- Run `cargo run -p nas-csi-xtask -- check` before finalizing changes.
- Do not commit `.nas-csi`, real host config, secrets, kubeconfigs, or build
  output.
