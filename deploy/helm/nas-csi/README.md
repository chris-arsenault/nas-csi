# deploy/helm/nas-csi

Placeholder for a future Helm chart for Kubernetes-side deployment.

The current installable substrate manifest is
[`../../kubernetes/nas-csi/nas-csi.yaml`](../../kubernetes/nas-csi/nas-csi.yaml).

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
