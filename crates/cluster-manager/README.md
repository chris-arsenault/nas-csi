# crates/cluster-manager

k3s cluster lifecycle library used by `nas-csi-host-agent`.

Responsibilities:

- Render k3s server and agent configuration.
- Generate cluster bootstrap cloud-init fragments.
- Manage cluster token and kubeconfig state.
- Plan ordered first-server startup after VM creation.
- Plan additional server and agent joins.
- Wait for guest k3s, Kubernetes API, and node readiness.
- Reconcile node labels and taints.
- Reconcile substrate add-ons, including `nas-csi`, with manifest hash markers.
- Drain, cordon, uncordon, restart, and rebuild nodes for maintenance.
- Plan controlled k3s upgrades.

This crate must not deploy user application workloads.

Primary profiles:

- `maintenance-basic`: one server and two or more agents;
- `maintenance-control-plane`: three servers with embedded etcd.
