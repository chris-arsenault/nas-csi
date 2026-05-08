# deploy/kubernetes/nas-csi

Static Kubernetes substrate manifests for the `nas-csi` driver.

`nas-csi.yaml` is the manifest reconciled by:

```sh
nas-csi-host-agent cluster apply --config /etc/nas-csi/host.yaml --execute
```

It contains only driver infrastructure: RBAC, `CSIDriver`, StorageClasses, the
controller Deployment, and the node DaemonSet. It intentionally does not create
application workloads or example PVCs.

Example PV/PVC files live under `examples/` and are for manual testing.
