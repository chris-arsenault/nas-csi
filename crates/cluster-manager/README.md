# crates/cluster-manager

k3s cluster lifecycle library used by `nas-csi-host-agent`.

Responsibilities:

- Render k3s server and agent configuration.
- Generate cluster bootstrap cloud-init fragments.
- Manage cluster token and kubeconfig state.
- Bootstrap the first server.
- Join additional server and agent nodes.
- Wait for Kubernetes API and node readiness.
- Reconcile substrate add-ons, including `nas-csi`.
- Drain, cordon, uncordon, restart, and rebuild nodes for maintenance.
- Plan controlled k3s upgrades.

This crate must not deploy user application workloads.

Primary profiles:

- `maintenance-basic`: one server and two or more agents;
- `maintenance-control-plane`: three servers with embedded etcd.
