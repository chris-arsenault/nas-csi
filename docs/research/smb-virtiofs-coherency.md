# SMB And Virtiofs Coherency

## Core Problem

The same host filesystem tree is accessed through two paths:

```text
SMB client -> Samba -> ZFS dataset
Kubernetes pod -> Linux guest VFS -> virtiofs -> virtiofsd -> ZFS dataset
```

This is correct only if the workload accepts shared-filesystem semantics. It is
not transactional. CSI can mount the dataset, but it cannot prevent two users or
two applications from making conflicting edits.

## Repository Dataset Policy

The repository dataset is the hardest workload because it has:

- many small files;
- frequent metadata operations;
- lock files;
- package-manager churn;
- possible SMB edits from other machines.

Initial policy:

- no guest writeback cache;
- `cache=auto` for benchmark, with `cache=none` strict mode;
- locks enabled;
- xattrs enabled;
- no pod-level read-only restriction;
- application-level convention that active builds should not race active SMB
  edits in the same checkout.

Tests:

- SMB edit appears in guest.
- Guest edit appears over SMB.
- simultaneous rename does not corrupt filesystem state.
- git lock files behave correctly.
- `npm install` does not exhaust host-agent or `virtiofsd` resources.

## Samples Dataset Policy

The samples dataset is mostly read-only from Kubernetes and writable over SMB
for content management.

Initial policy:

- Kubernetes publish is read-only.
- SMB remains writable.
- content update is an explicit publish event.
- streaming server has a rescan/reload path.
- more aggressive cache mode can be benchmarked because Kubernetes does not
  write to the dataset.

Tests:

- pod cannot write;
- SMB can update;
- server sees update after rescan or remount;
- old files remain readable during a large library update;
- rename-based publish works.

## Things CSI Cannot Solve

- It cannot make concurrent edits logically safe.
- It cannot make SMB client oplocks and Linux guest file caching a single
  distributed lock manager.
- It cannot repair an application that assumes exclusive local disk semantics.

The design can expose the same dataset safely enough for the target workflows,
but the policy must be explicit per dataset.

Sources:

- <https://www.truenas.com/docs/scale/shares/smb/>
- <https://virtio-fs.gitlab.io/design.html>
- <https://virtio-fs.gitlab.io/qemu/tools/virtiofsd.html>
