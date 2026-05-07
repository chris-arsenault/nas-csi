# Cluster Rebuild

## Goal

Recreate the substrate cluster from desired state without deleting TrueNAS
application datasets.

## Preserved State

The rebuild must preserve:

- SMB-visible datasets;
- ZFS snapshots;
- replication and retention tasks;
- host-agent config;
- k3s token and kubeconfig backups when possible;
- workload manifests owned by the user or GitOps system.

## Disposable State

The rebuild may replace:

- node VM root disks;
- cloud-init seed images;
- libvirt domains;
- `virtiofsd` sockets;
- k3s cluster runtime state;
- CSI deployment resources.

## Flow

1. Export current host-agent status.
2. Confirm desired config parses.
3. Confirm TrueNAS datasets exist.
4. Stop Kubernetes workloads through the user's normal deployment tool if needed.
5. Destroy node VMs with dataset deletion disabled.
6. Recreate node VMs from desired state.
7. Bootstrap k3s.
8. Reinstall substrate add-ons and `nas-csi`.
9. Verify CSI can mount selected read-write and read-only datasets.
10. Return application deployment to the user/GitOps system.

## Non-Goals

This project does not restore application workloads by itself. It restores the
cluster substrate and storage integration.
