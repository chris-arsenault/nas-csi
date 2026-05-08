# Deployment Scope

`nas-csi` deliberately starts with a narrow target: one TrueNAS host, local
libvirt/KVM node VMs, k3s inside those VMs, and existing TrueNAS filesystem
datasets exposed through virtiofs and CSI.

## First Deployment Rules

- Keep the initial deployment to one TrueNAS host and IaC-owned VM-based k3s.
- Keep host-specific facts, generated configs, secrets, image checksums, tokens,
  and kubeconfigs out of the repo.
- Make static existing-dataset CSI the first storage deployment target.
- Treat dynamic dataset provisioning as an explicitly opted-in controller
  feature, not the default deployment path.
- Do not fork or patch `virtiofsd` until a target-host failure is reproducible
  and captured by workload validation.
- Do not add HA or multi-host behavior beyond the maintenance use case of
  rebuilding or updating one VM node at a time.

## Real Workloads

The project exists to support two concrete workload classes:

- SMB-visible git repository datasets used from Kubernetes for small-file build
  workloads.
- SMB-managed read-only VST/Kontakt content served by a Kubernetes workload.

These datasets remain normal TrueNAS datasets. TrueNAS owns SMB shares,
snapshots, replication, quotas, and retention. Kubernetes sees those datasets
through static existing-dataset PV/PVC objects and the CSI node plugin, but it
does not become the authoritative storage owner.

## Non-Goals

- Multi-host Kubernetes HA.
- Replacing SMB access.
- Moving authoritative dataset ownership into Kubernetes.
- TrueNAS UI registration as a deployment blocker.
- Dynamic PVCs as the default first-deploy path.
- Controller-managed retention or replication policy.
- A `virtiofsd` fork before there is a reproducible target-host failure.
- A broad hardening framework unrelated to first deployment or dataset safety.
