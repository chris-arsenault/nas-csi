# deploy/helm/nas-csi

Helm chart for Kubernetes-side deployment.

Planned resources:

- CSI controller Deployment.
- CSI node DaemonSet.
- `CSIDriver` object.
- RBAC for CSI sidecars.
- StorageClasses for `repos-dev` and `samples-ro`.
- Example PersistentVolumes and PersistentVolumeClaims.

The host agent is not deployed by this chart because it runs on the TrueNAS host,
outside Kubernetes.

This chart is substrate. It must not include application workloads.
