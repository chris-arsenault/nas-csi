# Cluster Management

## Position

It is viable and coherent for this project to own the substrate Kubernetes
cluster.

The ownership model becomes:

```text
TrueNAS
  owns pools, filesystem datasets, SMB, snapshots, replication, retention

nas-csi-host-agent
  owns node VMs, VM root disks, cloud-init, k3s bootstrap, cluster config,
  virtiofsd, virtiofs device wiring, and CSI installation

Kubernetes
  owns pod scheduling, PV/PVC binding, system controllers, and runtime state

User/GitOps
  owns application workloads
```

This is a good boundary because the storage integration depends on the VM
definition and the Kubernetes node identity. Owning the cluster lets the project
make those identities deterministic instead of trying to bolt CSI onto an opaque
cluster later.

## Scope

In scope:

- create and maintain node VMs;
- install a pinned k3s version;
- configure server and agent nodes;
- manage the cluster token;
- retrieve and store kubeconfig;
- install the `nas-csi` controller and node plugin;
- install required infrastructure add-ons;
- reconcile node labels and taints;
- support node rebuilds;
- support cluster upgrades and rollback planning;
- back up cluster state.

Out of scope:

- deploying user applications;
- owning app Helm releases;
- managing app secrets;
- choosing application ingress/routes;
- replacing a GitOps system.

System add-ons are not application workloads in this boundary. CSI, CNI,
CoreDNS, metrics-server, and optional infrastructure controllers are part of the
cluster substrate.

## Distribution Choice

Use k3s for v1.

Reasons:

- single binary distribution;
- simple server/agent model;
- declarative config file at `/etc/rancher/k3s/config.yaml`;
- straightforward cloud-init bootstrap;
- embedded SQLite for a single server or embedded etcd for 3+ servers;
- kubeconfig lives at `/etc/rancher/k3s/k3s.yaml`;
- packaged components can be kept or disabled explicitly.

Kubeadm can be a later backend if you need full upstream Kubernetes mechanics.
It adds more moving parts: container runtime installation, kubelet/kubeadm
version matching, CNI installation, certificate handling, and join-token
workflow. That is unnecessary for the first target.

## Desired State

The generated local host config has a cluster section like this. The values below
are placeholders; setup discovers or generates the concrete values on the target
host.

```yaml
cluster:
  name: <generated-or-selected-name>
  distribution: k3s
  version: <selected-k3s-version>
  apiServer:
    endpoint: https://<discovered-or-selected-endpoint>:6443
    tlsSans:
      - <selected-dns-name>
      - <discovered-lan-ip>
  tokenFile: <local-token-path>
  kubeconfigOut: <local-kubeconfig-path>
  network:
    clusterCidr: 10.42.0.0/16
    serviceCidr: 10.43.0.0/16
    clusterDns: 10.43.0.10
    flannelBackend: vxlan
  disable:
    - traefik
    - servicelb
  addons:
    nasCsi:
      enabled: true
    metricsServer:
      enabled: true

nodes:
  - name: k3s-1
    role: server
    k3s:
      clusterInit: true
      labels:
        nas-csi.dev/storage-node: "true"
      taints: []
  - name: k3s-2
    role: agent
    k3s:
      server: https://<discovered-or-selected-endpoint>:6443
      labels:
        nas-csi.dev/storage-node: "true"
```

## Maintenance-Oriented Multi-Node

Multi-node is useful here, but the goal is controlled maintenance, not physical
HA.

All node VMs still live on one TrueNAS host. If that host, pool, bridge, or power
domain fails, the cluster fails. The benefit of multiple Kubernetes nodes is that
the host agent can drain and rebuild one VM at a time while other VMs keep
running workloads.

Recommended profiles:

`maintenance-basic`
: One k3s server VM and two or more agent VMs. Run application workloads on
  agents. This supports rolling agent maintenance. The Kubernetes API is down
  during server VM maintenance, but already-running workloads can continue.

`maintenance-control-plane`
: Three k3s server VMs with embedded etcd and optional agent VMs. This supports
  rolling server VM maintenance without intentionally taking down the Kubernetes
  API. It is more complex and still not physical HA because all servers are on
  the same TrueNAS host.

Downtimeless application updates require normal Kubernetes application design:
multiple replicas, readiness probes, disruption budgets, and storage access modes
that allow the workload to move. This project can drain nodes correctly, but it
cannot make a single-replica app downtime-free.

Example desired-state files:

- [maintenance-basic](../../examples/intents/maintenance-basic.yaml)
- [maintenance-control-plane](../../examples/intents/maintenance-control-plane.yaml)

## Bootstrap Flow

1. Host agent validates TrueNAS datasets and host network.
2. Host agent creates VM root disks and cloud-init seeds.
3. First server VM boots with `cluster-init: true`.
4. Host agent waits for SSH or qemu guest agent readiness.
5. Host agent waits for k3s API readiness.
6. Host agent retrieves kubeconfig from `/etc/rancher/k3s/k3s.yaml`.
7. Host agent rewrites kubeconfig server endpoint to the configured LAN/API
   endpoint.
8. Additional server or agent VMs boot with the shared token and server URL.
9. Host agent waits for nodes to become Ready.
10. Host agent installs or reconciles infrastructure add-ons.
11. Host agent installs or reconciles `nas-csi`.
12. Host agent reports cluster health.

## Node Bootstrap

Cloud-init should render:

- hostname;
- SSH keys;
- qemu guest agent installation;
- k3s install source and pinned version;
- `/etc/rancher/k3s/config.yaml`;
- `/etc/nas-csi/node.yaml`;
- expected virtiofs mount tags;
- optional registries config;
- systemd units for early mount checks if needed.

Prefer rendering k3s config files over long shell command lines. k3s supports
configuration through `/etc/rancher/k3s/config.yaml` and drop-in files.

## Secrets And State

Host-agent-managed cluster state:

- k3s token;
- kubeconfig;
- per-node cloud-init seed;
- VM root disk metadata;
- cluster desired-state hash;
- installed k3s version;
- installed CSI chart version.

Store secrets under `/etc/nas-csi` with restrictive permissions. Do not store
them in the repo.

K3s server datastore:

- single-server mode uses k3s default local datastore;
- HA mode uses embedded etcd on 3+ server nodes;
- datastore backups must be part of the host-agent maintenance plan.

TrueNAS ZFS snapshots of VM root disk datasets are useful, but they are not a
substitute for an explicit k3s datastore backup and restore procedure.

## Add-On Boundary

Install only cluster substrate add-ons:

- `nas-csi`;
- metrics-server if not already supplied;
- optional CNI replacement if flannel is disabled;
- optional load balancer controller such as MetalLB if the cluster API or
  ingress needs it;
- optional ingress controller only if it is part of the substrate.

Do not install application Helm charts from this project.

## Reconciliation

The cluster manager should be state-aware:

- If the cluster does not exist, bootstrap it.
- If the cluster exists and desired state matches, do nothing.
- If a node VM is missing, rebuild and rejoin it.
- If a node is NotReady, report health before destructive action.
- If k3s config changed, plan a node restart.
- If server membership changes, handle quorum deliberately.
- If k3s version changes, perform ordered upgrades.
- If the CSI chart changed, reconcile it after the API is healthy.

For v1, prefer explicit commands:

- `plan`
- `apply`
- `status`
- `reconcile`
- `upgrade`
- `drain-node`
- `uncordon-node`
- `destroy-node`
- `rebuild-node`
- `destroy-cluster`

`destroy-cluster` must not delete TrueNAS application datasets.

## Upgrade Model

Pin versions:

- base OS image;
- k3s version;
- CSI chart version;
- host-agent version;
- virtiofsd version.

Upgrade order:

1. host-agent;
2. one node VM root image or package baseline at a time;
3. k3s servers;
4. k3s agents;
5. CSI chart;
6. optional substrate add-ons.

On a single physical host this is about controlled maintenance, not high
availability.

## Node Maintenance Flow

The host agent should implement node maintenance as a planned workflow:

1. Verify the Kubernetes API is healthy.
2. Verify replacement capacity exists.
3. Cordon the node.
4. Drain the node with a configurable timeout and disruption policy.
5. Stop workloads that cannot be evicted only when explicitly allowed.
6. Stop or rebuild the VM.
7. Start the VM and wait for guest readiness.
8. Wait for k3s to rejoin.
9. Verify virtiofs exports and CSI node plugin health.
10. Uncordon the node.

For server nodes in `maintenance-control-plane`, the workflow must also verify
etcd quorum before and after each step. For `maintenance-basic`, server node
maintenance is a planned control-plane outage.

## Failure Model

Important failures:

- first server VM lost;
- k3s token lost;
- kubeconfig lost;
- node root disk corrupted;
- virtiofsd unhealthy;
- cluster API unavailable;
- etcd quorum lost in multi-server mode;
- host reboot during reconciliation.

Design response:

- keep node root disks disposable;
- keep cluster token and kubeconfig backed up;
- snapshot VM state datasets before planned upgrades;
- keep TrueNAS app datasets outside cluster destroy operations;
- require explicit confirmation before cluster-wide destructive operations;
- make host-agent operations idempotent.

## Implementation Impact

Add a `cluster-manager` library used by the host agent.

Responsibilities:

- render k3s config;
- render cloud-init cluster bootstrap content;
- manage cluster token and kubeconfig;
- run remote readiness checks through SSH or qemu guest agent;
- call Kubernetes API after bootstrap;
- install/reconcile the `nas-csi` Helm chart;
- report cluster/node health.

The host agent remains the only TrueNAS-side daemon. VM, cluster, and storage
logic should be separate Rust libraries under it.

Sources:

- <https://docs.k3s.io/installation/configuration>
- <https://docs.k3s.io/datastore/ha-embedded>
- <https://docs.k3s.io/cluster-access>
- <https://docs.k3s.io/architecture>
- <https://kubernetes.io/docs/reference/setup-tools/kubeadm/kubeadm-init/>
- <https://kubernetes.io/docs/reference/setup-tools/kubeadm/kubeadm-join/>
