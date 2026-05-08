# Configuration

`nas-csi-host-agent` is driven by a generated local desired-state config file.

The concrete config describes:

- TrueNAS API endpoint and API key path;
- discovered host tool paths for `virsh`, `qemu-img`, `virtiofsd`, and
  `systemctl`;
- libvirt connection and bridge;
- image and VM state datasets;
- k3s cluster version and profile;
- node VM resources and roles;
- root disk base image path, format, and checksum;
- dataset exports and policies;
- substrate add-ons.

Concrete config is generated on the target host by the discovery/onboarding
flow:

1. `ClusterIntent` stays repo-safe and describes profile, node counts, storage
   policy names, and substrate add-ons.
2. `DiscoveryInventory` is read-only host inventory from the target TrueNAS
   host.
3. `HostSelections` is local-only operator input for choices discovery cannot
   decide, such as bridge, selected datasets, VM sizing, and token paths.
4. `HostConfig` is the generated desired state used for plan, render, and later
   apply.

`HostSelections`, `DiscoveryInventory`, and `HostConfig` should live under
`/etc/nas-csi`, `.nas-csi`, or another local path and should not be committed
with real values.

Examples under [examples](../../examples/README.md) are fictional fixtures, not
complete host configs for a real machine.

## Profiles

`maintenance-basic`
: One k3s server and two or more agents. Prefer this first. It gives rolling
  worker maintenance with lower cluster complexity.

`maintenance-control-plane`
: Three k3s servers using embedded etcd. Use this when server VM updates should
  avoid planned Kubernetes API downtime. It is still one physical-host failure
  domain.

## Destructive Safety

Config-driven destroy operations must distinguish between disposable and
authoritative state.

Disposable:

- node VM root disks;
- cloud-init seeds;
- libvirt domains;
- k3s runtime state;
- transient virtiofs sockets.

Authoritative:

- TrueNAS application datasets;
- SMB shares;
- ZFS snapshots;
- replication and retention tasks.

The host agent must require explicit opt-in before deleting authoritative
storage objects.

## Image Integrity

`HostSelections.image` carries the operator-selected base cloud image path and
an optional SHA-256 checksum:

```yaml
image:
  source: /mnt/<pool>/nas-csi/images/debian.qcow2
  checksum: sha256:<64-hex>
```

Materialization copies that value into each node `rootDisk.sourceChecksum`.
Before creating a missing root disk overlay, reconcile verifies that
`rootDisk.sourceImage` exists, that `qemu-img info` reports the configured
`rootDisk.sourceFormat`, and that the file SHA-256 matches
`rootDisk.sourceChecksum`. A missing checksum is a refusal for backed root disk
creation because the base image is executable VM state.

## Repo Boundary

Allowed in repo:

- cluster profile;
- node counts;
- workload policy names;
- add-on intent;
- docs and schemas.

Not allowed in repo:

- real pool names;
- real dataset names;
- LAN IPs or DNS names;
- MAC addresses;
- API key paths that imply a deployment;
- kubeconfig paths;
- k3s tokens;
- SSH public keys;
- generated VM domain names.

## Runtime Node Contract

Each VM receives `/etc/nas-csi/node.yaml` as a `NodeRuntimeConfig` containing
only the exports assigned to that VM, their virtiofs tags, and the guest mount
paths under `/var/lib/nas-csi/virtiofs`. Cloud-init can seed the initial file,
and `nas-csi-host-agent csi install --execute` refreshes it through the qemu
guest agent before the CSI node DaemonSet is verified.

The CSI node plugin treats this file as authoritative. If a requested volume is
not present there, or the matching virtiofs mount is absent, the node operation
fails closed.
