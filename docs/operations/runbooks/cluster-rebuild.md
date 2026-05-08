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
3. Confirm TrueNAS datasets, SMB shares, snapshots, replication, quotas, and
   retention tasks still exist in TrueNAS.
4. Stop Kubernetes workloads through the user's normal deployment tool if needed.
5. Remove or replace disposable node VM state using an explicit operator
   procedure.
6. Run `host-install --post-reboot-check` if the host substrate should already
   be present, or `host-install --execute` to recreate VM/runtime substrate from
   local desired state.
7. Run `cluster install --execute` to bootstrap k3s and reinstall substrate
   manifests.
8. Run `csi install --execute` to recreate static existing-dataset PV/PVCs and
   smoke test mounts.
9. Run `workload validate --execute` for repository/content datasets when the
   storage path is expected to be production-ready.
10. Return application deployment to the user/GitOps system.

The current implementation does not provide a single `destroy-cluster` command.
Keep the destructive steps explicit until that workflow has dedicated guards.

## Non-Goals

This project does not restore application workloads by itself. It restores the
cluster substrate and storage integration.
