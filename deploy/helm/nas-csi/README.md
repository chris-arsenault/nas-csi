# deploy/helm/nas-csi

Reserved location for a future Helm chart wrapper for Kubernetes-side
deployment.

The current installable substrate manifest is
[`../../kubernetes/nas-csi/nas-csi.yaml`](../../kubernetes/nas-csi/nas-csi.yaml).

The static manifest already carries the controller Deployment, node DaemonSet,
RBAC, `CSIDriver`, StorageClasses, and examples. A future chart should package
that same substrate without introducing application workloads.

The host agent is not deployed by this chart because it runs on the TrueNAS host,
outside Kubernetes.

This chart is substrate. It must not include application workloads.
