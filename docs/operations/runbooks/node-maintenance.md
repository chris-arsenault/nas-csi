# Node Maintenance

## Goal

Restart, update, or rebuild one Kubernetes node VM while preserving authoritative
TrueNAS datasets.

This is maintenance continuity, not physical HA. All nodes still depend on the
same TrueNAS host.

## Current Command Support

The current host-agent CLI supports status and reconciliation checks:

```sh
nas-csi-host-agent status --config /etc/nas-csi/host.yaml
nas-csi-host-agent health --config /etc/nas-csi/host.yaml
nas-csi-host-agent cluster status --config /etc/nas-csi/host.yaml
nas-csi-host-agent cluster plan --config /etc/nas-csi/host.yaml
```

Dedicated `drain-node`, `restart-node`, `rebuild-node`, and `uncordon-node`
commands are planned maintenance workflow work. Until they exist, use
Kubernetes-native drain/cordon commands and host-agent status checks.

## Preconditions

- Kubernetes API is reachable.
- Host-agent health is clean or understood.
- TrueNAS datasets are mounted.
- Expected virtiofs sockets exist.
- The target node is not the only node that can run affected workloads.
- Workloads have enough replicas or tolerate disruption.

## Agent Node Flow

1. Capture `status`, `health`, and `cluster status` output.
2. Cordon and drain the node with `kubectl`.
3. Stop or restart the VM using the current host/operator mechanism.
4. Run host `apply --execute` if VM/runtime desired state must be repaired.
5. Run `cluster apply --execute` if k3s join or labels/taints must be repaired.
6. Wait for the Kubernetes node to become Ready.
7. Verify `nas-csi-node` is running on the node.
8. Smoke test CSI staging for the affected dataset policy.
9. Uncordon the node with `kubectl`.

## Server Node Flow

For `maintenance-basic`, server maintenance is a planned Kubernetes API outage.
Already-running workloads may continue, but scheduling and reconciliation pause
until the server returns.

For `maintenance-control-plane`, update one server at a time and verify etcd
quorum before and after the VM change.

## Storage Checks

Before uncordoning:

- the TrueNAS dataset is mounted on the host;
- the host-side virtiofs socket exists;
- the guest virtiofs mount exists under `/var/lib/nas-csi/virtiofs`;
- the CSI node plugin can stage the volume;
- read-only exports remain read-only;
- read-write exports pass a small create/read/delete smoke test if policy
  allows it.

## Abort Conditions

Stop the workflow if:

- the Kubernetes API is unavailable before starting;
- draining would violate disruption policy;
- `virtiofsd` is unhealthy;
- etcd quorum is at risk;
- the node rejoins with the wrong identity;
- a selected dataset mount is missing on TrueNAS.
