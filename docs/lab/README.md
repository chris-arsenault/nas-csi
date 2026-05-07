# Lab Plan

The lab proves the transport before CSI.

## Required Inventory

- TrueNAS version.
- Kernel version.
- QEMU version.
- libvirt version.
- `virtiofsd` path and version.
- LAN bridge name for node VMs.
- Kubernetes node VM name and libvirt domain name.
- base cloud image path and checksum.
- k3s version and install source.
- cluster token storage path.
- selected cluster profile: `maintenance-basic` or
  `maintenance-control-plane`.
- Dataset names and SMB share paths.

## Benchmarks

Repos dataset:

- `git status`
- `git checkout`
- `npm install`
- package manager cache warm/cold runs
- clean build
- SMB edit visibility test

Samples dataset:

- sequential read throughput
- random small read latency
- application-level streaming test
- SMB-side library update followed by server rescan

## Failure Tests

- Destroy and rebuild node VM from desired state.
- Destroy and rebuild k3s agent from desired state.
- Drain and uncordon a node.
- Restore kubeconfig from host-agent state.
- Restart node VM.
- Restart host-agent.
- Kill `virtiofsd`.
- Restart SMB.
- Take TrueNAS snapshot during workload.
- Fill dataset quota.
- Remove virtiofs mount tag and verify node plugin fails closed.
