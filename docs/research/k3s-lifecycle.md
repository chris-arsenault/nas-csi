# k3s Cluster Lifecycle

## Research Finding

k3s is a good v1 Kubernetes distribution target for this project.

The official docs support the lifecycle this project needs:

- k3s can be installed as a system service;
- k3s can be configured through `/etc/rancher/k3s/config.yaml`;
- server and agent nodes use explicit server URL and token-based join;
- the admin kubeconfig is written to `/etc/rancher/k3s/k3s.yaml`;
- HA embedded etcd is supported with 3+ server nodes;
- critical server flags must match across server nodes.

That maps well to host-agent-generated cloud-init and declarative node desired
state.

## Initial Cluster Shape

Start with one server VM plus optional agent VMs for `maintenance-basic`.

Reasons:

- all nodes share one physical TrueNAS failure domain;
- single-server mode is much simpler to bootstrap and recover;
- application data is not in the cluster datastore;
- this project needs reliable storage mapping more than control-plane HA.

Later, support 3 server VMs with embedded etcd for
`maintenance-control-plane` if controlled server upgrades and API maintenance
become important. Treat that as maintenance convenience, not real physical HA.

## Configuration Rules

Generate k3s config files instead of long installer command lines.

Server config should include:

- token;
- TLS SANs;
- cluster/service CIDRs;
- disabled packaged components;
- node labels and taints;
- `cluster-init: true` for the first server;
- `server: https://...:6443` for additional servers.

Agent config should include:

- token;
- server URL;
- node labels and taints.

Critical server flags must be consistent across server nodes.

## Host-Agent Responsibilities

The host agent should:

- generate token if missing;
- render cloud-init content;
- wait for node boot;
- wait for k3s API readiness;
- retrieve kubeconfig;
- rewrite kubeconfig endpoint;
- join nodes;
- reconcile labels and taints;
- install substrate add-ons;
- back up datastore state before upgrades.

## Open Questions

- Which exact k3s version should be pinned?
- Should the first cluster use default flannel or a custom CNI?
- Should servicelb and Traefik be disabled by default?
- Should the control-plane endpoint be a LAN DNS record, static IP, or later a
  load balancer?
- Should the host agent install k3s from the official script, a pinned binary,
  or an air-gapped artifact cache?

Sources:

- <https://docs.k3s.io/installation/configuration>
- <https://docs.k3s.io/datastore/ha-embedded>
- <https://docs.k3s.io/cluster-access>
- <https://docs.k3s.io/architecture>
