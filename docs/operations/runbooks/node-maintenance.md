# Node Maintenance

## Goal

Update, rebuild, or restart one Kubernetes node VM while keeping the rest of the
cluster useful.

This is maintenance continuity, not physical HA. All nodes are still on the same
TrueNAS host.

## Preconditions

- Kubernetes API is reachable.
- Host agent is healthy.
- TrueNAS datasets are mounted.
- `virtiofsd` exports are healthy.
- The target node is not the only node that can run the affected workloads.
- Application workloads have enough replicas or tolerate disruption.

## Agent Node Flow

1. `nas-csi-host-agent status`
2. `nas-csi-host-agent drain-node <node>`
3. Wait for pods to evict.
4. `nas-csi-host-agent rebuild-node <node>` or `restart-node <node>`.
5. Wait for VM boot and qemu guest agent readiness.
6. Wait for k3s node Ready.
7. Wait for `nas-csi-node-plugin` Ready.
8. `nas-csi-host-agent uncordon-node <node>`
9. Verify workload placement.

## Server Node Flow

For `maintenance-basic`, server node maintenance is a planned Kubernetes API
outage. Already-running workloads may continue, but scheduling and reconciliation
pause until the server returns.

For `maintenance-control-plane`, update one server at a time:

1. Verify etcd quorum.
2. Cordon and drain if the server can run workloads.
3. Restart or rebuild the server VM.
4. Wait for k3s server Ready.
5. Verify etcd membership and quorum.
6. Continue to the next server only after the cluster is healthy.

## Storage Checks

Before uncordoning:

- expected virtiofs mount tags exist in the VM;
- CSI node plugin can stage volumes;
- read-only exports remain read-only;
- repo exports pass a small create/read/delete smoke test if policy allows it.

## Abort Conditions

Stop the workflow if:

- Kubernetes API is unavailable before starting;
- draining would violate disruption policy;
- `virtiofsd` is unhealthy;
- etcd quorum is at risk;
- the node rejoins with the wrong identity;
- a dataset mount is missing on TrueNAS.
