# Cluster Management

## Position

The host agent owns the substrate k3s cluster because the storage transport
depends on deterministic VM identity, Kubernetes node identity, and virtiofs
export identity.

This ownership is limited to cluster infrastructure. It does not include user
applications, app secrets, ingress routes, app databases, or GitOps state.

## Scope

In scope:

- a pinned k3s distribution version;
- first-server bootstrap;
- additional server and agent joins;
- k3s token lifecycle;
- kubeconfig retrieval and endpoint rewriting;
- Kubernetes API and node readiness checks;
- node label and taint reconciliation;
- substrate add-ons such as metrics-server;
- `nas-csi` controller and node plugin installation;
- status reporting for the substrate.

Out of scope:

- application workload deployment;
- app Helm releases;
- user secrets;
- physical high availability;
- broad Kubernetes distribution support beyond k3s.

## Profiles

`maintenance-basic`
: One server VM plus agent VMs. This supports rolling agent maintenance. Server
  maintenance is a planned Kubernetes API outage, though already-running
  workloads may continue.

`maintenance-control-plane`
: Three server VMs with embedded etcd. This supports rolling server maintenance
  without intentionally taking the Kubernetes API down. It is still one
  physical-host failure domain.

Multi-node is a maintenance tool, not a claim that the cluster survives loss of
the TrueNAS host, pool, bridge, or power domain.

## Bootstrap Flow

The implemented flow assumes VM/runtime `apply` has already rendered and
reconciled domains, seed images, and virtiofsd services.

1. Ensure the k3s token file exists with restrictive permissions.
2. Start the first server domain if it is not running.
3. Wait for guest k3s readiness through the QEMU guest agent.
4. Retrieve `/etc/rancher/k3s/k3s.yaml` from the first server.
5. Rewrite the kubeconfig server endpoint to the configured API endpoint.
6. Wait for Kubernetes API readiness from the host.
7. Start additional server and agent domains.
8. Wait for Kubernetes nodes to become Ready.
9. Reconcile desired node labels and taints.
10. Apply configured substrate manifests.
11. Run `csi install` to refresh node runtime config, create static existing
    dataset PV/PVCs, and verify CSI mount behavior.

The CLI surface is:

```sh
nas-csi-host-agent cluster plan --config /etc/nas-csi/host.yaml
nas-csi-host-agent cluster apply --config /etc/nas-csi/host.yaml --execute
nas-csi-host-agent cluster status --config /etc/nas-csi/host.yaml
```

Package installs place manifests under `/usr/local/share/nas-csi/deploy`; repo
development can use `--manifest-root deploy`.

## Node Bootstrap

Cloud-init renders the initial guest-side substrate contract:

- hostname;
- qemu guest agent installation;
- pinned k3s install command and config file;
- k3s token file;
- `/etc/nas-csi/node.yaml`;
- virtiofs fstab entries under `/var/lib/nas-csi/virtiofs`.

The CSI installer refreshes `/etc/nas-csi/node.yaml` through the qemu guest
agent before applying and verifying the node DaemonSet. That keeps node runtime
config tied to the host-local `HostConfig` even after VM rebuilds or export
selection changes.

The node plugin treats `/etc/nas-csi/node.yaml` and the guest mount table as
authoritative. Missing exports fail closed.

## Secrets And State

Host-local cluster state includes:

- k3s token;
- kubeconfig;
- rendered artifacts;
- cloud-init seed images;
- applied-manifest hash markers.

These files belong under local host paths such as `/etc/nas-csi` and
`/var/lib/nas-csi`. They must not be committed to the repository.

K3s datastore backups remain an operational requirement. TrueNAS snapshots of
VM state datasets are useful, but they are not a substitute for explicit k3s
datastore backup and restore testing.

## Add-On Boundary

The host agent applies cluster substrate only:

- `nas-csi`;
- metrics-server when enabled;
- optional CNI or load balancer infrastructure if later added as substrate.

The manifest root must not contain user workloads in the path reconciled by the
host agent. Example PV/PVC manifests are kept separately for manual testing.

## Reconciliation Semantics

The cluster manager compares desired state with observed state and emits typed
operations. It skips already-current token, kubeconfig, node, label, taint, and
manifest state. It applies only the missing or divergent substrate steps.

The implementation intentionally uses `program + argv` command specs for host
commands. Guest readiness and kubeconfig retrieval use the QEMU guest agent
rather than assuming SSH access.

## Future Maintenance Work

The current cluster commands bootstrap and reconcile substrate. Planned
maintenance commands should build on the same state model:

- drain and uncordon a node;
- restart or rebuild one node VM;
- upgrade k3s servers and agents in order;
- verify etcd quorum for `maintenance-control-plane`;
- destroy and rebuild cluster runtime without touching TrueNAS app datasets.

Until those commands exist, runbooks should treat node maintenance as a manual
operator workflow supported by `status`, `health`, and `cluster status`.
