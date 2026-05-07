# Changelog

All notable project changes should be recorded here.

This project uses a simple human-readable changelog while the API and release
process are still early.

## Unreleased

### Added

- Rust workspace with host-agent, VM manager, cluster manager, discovery,
  TrueNAS client, CSI driver, CSI node plugin, shared types, and xtask crates.
- Repo-safe intent, discovery, selections, and generated host config model.
- Host-side VM artifact rendering for libvirt domain XML, k3s config,
  cloud-init NoCloud data, and virtiofsd systemd units.
- Internal Rust VFAT writer for cloud-init NoCloud seed images.
- State-aware host reconciliation planner with `apply`, `skip`, and `refuse`
  decisions.
- Discovered host tool paths in generated `HostConfig.hostTools`.
- Initial documentation set covering architecture, VM ownership, k3s lifecycle,
  discovery, configuration, research, and operational runbooks.

### Notes

- `nas-csi-host-agent apply` defaults to dry-run. The `--execute` path exists
  but should be treated as early integration code until tested on the target
  TrueNAS host.
