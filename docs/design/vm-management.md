# VM Management

## Position

`nas-csi-host-agent` owns Kubernetes node VMs directly through libvirt/QEMU.
That is the cleanest boundary for virtiofs because shared memory backing,
filesystem devices, externally launched `virtiofsd` sockets, mount tags, root
disks, and cloud-init all have to agree.

TrueNAS UI-created VMs are not the preferred model. They make middleware and
the host agent compete over VM definition details.

## Ownership Boundary

TrueNAS owns datasets, SMB, snapshots, replication, quotas, retention, and
appliance services.

The host agent owns node VM definitions, root disk overlays, cloud-init seed
images, libvirt metadata, autostart policy, virtiofs devices, and `virtiofsd`
systemd units.

Kubernetes owns workloads and in-guest CSI mount lifecycle.

## Desired State

Concrete VM desired state lives in generated `HostConfig`, not in repo docs.
It is produced from intent, discovery, and local selections. The config carries
the selected bridge, VM resources, base image path and checksum, root disk
paths, node roles, k3s config, and dataset exports.

The docs describe the ownership model. The Rust types and example fixtures are
the source of truth for exact field names.

## Domain Shape

Generated domains are intentionally boring:

- `qemu:///system`;
- Q35 machine type;
- host CPU mode selected by config;
- virtio root disk;
- virtio network on the selected bridge;
- serial console and QEMU guest agent channel;
- shared memory backing for virtiofs;
- one filesystem device per assigned dataset export.

The host agent writes a `nas-csi` metadata hash into managed domain XML so it
can compare desired and actual domain shape without relying on raw libvirt XML
byte equality.

## Root Disks

Node root disks are disposable. They should not contain authoritative app data.

The current reconciler creates per-node qcow2 overlays from a selected base
image, validates the base image path, format, and SHA-256 before creation, and
can grow existing overlays when desired size increases. It refuses destructive
replacement and shrink operations.

## Cloud-Init

The VM manager generates NoCloud seed images directly in Rust. The seed image
contains guest bootstrap data for:

- hostname;
- qemu guest agent;
- k3s role and config;
- k3s token file;
- `/etc/nas-csi/node.yaml`;
- guest virtiofs fstab entries.

The CSI install workflow refreshes `/etc/nas-csi/node.yaml` through the qemu
guest agent before verifying Kubernetes node-plugin behavior. Cloud-init remains
the bootstrap path; the installer is the reconciliation path for current
host-local export state.

The implementation avoids external image tools such as `genisoimage` or
`cloud-localds`.

## Host Apply

`nas-csi-host-agent apply` defaults to dry-run. It renders desired artifacts,
inspects actual host state, and reports typed `apply`, `skip`, or `refuse`
decisions.

The current apply path covers:

- rendered artifact writes;
- root disk parent directories;
- qcow2 root disk creation and growth;
- cloud-init seed images;
- virtiofsd systemd units;
- systemd reload/start/restart for virtiofsd;
- libvirt domain define/redefine/autostart;
- guarded execution with rollback for systemd unit and domain redefine changes.

Unsafe cases become refusals. Existing root disks are never replaced. Running
domain XML changes require an explicit flag. Unmanaged same-name domains require
explicit adoption.

## VM Start Policy

The VM apply planner has a start-domain operation, but normal host `apply`
still focuses on define/autostart/runtime substrate. Cluster reconciliation is
the path that starts k3s node domains in the correct bootstrap order.

This keeps VM definition safety separate from cluster bring-up ordering.

## TrueNAS UI Visibility

TrueNAS UI integration is a nice-to-have, not a requirement. The current design
does not assume host-agent-owned libvirt domains appear in the TrueNAS VM UI.

A future UI integration should be read-only or officially API-backed. Direct
middleware database writes are out of bounds.

## Destructive Operations

Destroying or rebuilding a node VM may remove disposable VM state:

- libvirt domain definition;
- node root disk;
- cloud-init seed;
- transient sockets;
- `virtiofsd` services for that node.

It must not delete SMB-visible application datasets, ZFS snapshots, replication
tasks, or TrueNAS shares unless a separate storage operation explicitly allows
that.
