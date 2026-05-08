# Rollback

Rollback removes or disables `nas-csi` substrate without deleting authoritative
TrueNAS datasets. Use this when the first deployment needs to be backed out or
when rebuilding the substrate from scratch is clearer than repair.

## Preserve

Do not delete or rewrite:

- exported TrueNAS datasets;
- SMB share definitions and permissions;
- ZFS snapshots;
- replication, retention, and quota configuration;
- application files inside exported datasets.

## Kubernetes Substrate

Remove validation pods first if they were kept for inspection:

```sh
kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  delete -f /var/lib/nas-csi/rendered/workload-validation/workload-pods.yaml \
  --ignore-not-found=true
```

Remove static PV/PVC and CSI substrate manifests:

```sh
kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  delete -f /var/lib/nas-csi/rendered/csi/static-existing-datasets.yaml \
  --ignore-not-found=true

kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  delete -f /usr/local/share/nas-csi/deploy/kubernetes/nas-csi/nas-csi.yaml \
  --ignore-not-found=true
```

The static PVs use `Retain`, so deleting Kubernetes objects must not delete the
TrueNAS datasets.

## Host Services

Stop managed virtiofsd services before removing VM definitions:

```sh
systemctl stop 'nascsi-virtiofsd-*'
```

Inspect host status before removing any disposable substrate:

```sh
nas-csi-host-agent status --config /etc/nas-csi/host.yaml
nas-csi-host-agent health --config /etc/nas-csi/host.yaml
```

The current implementation intentionally does not provide a broad
`destroy-everything` command. Remove disposable VM state only through explicit
operator steps and only after confirming the paths belong to node VMs or
rendered `nas-csi` artifacts.

## Disposable State

Disposable state may include:

- libvirt node VM definitions managed by `nas-csi`;
- node VM root disk overlays;
- cloud-init seed images;
- `/run/nas-csi` sockets;
- generated artifacts under `/var/lib/nas-csi/rendered`;
- k3s runtime state inside disposable node VMs.

Authoritative dataset paths under `/mnt/<pool>/...` are not disposable merely
because they are exported to Kubernetes.

## Rebuild After Rollback

After rollback, use [First Deploy](first-deploy.md) to recreate the substrate.
If only one node needs replacement, prefer [Node Maintenance](node-maintenance.md)
over a full rollback.
