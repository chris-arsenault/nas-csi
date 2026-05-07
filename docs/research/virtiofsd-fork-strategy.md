# virtiofsd Fork Strategy

## Baseline

Use upstream Rust `virtiofsd-rs` first. It is the active virtiofs daemon line and
is a practical fork point if the workload exposes defects.

Do not fork for convenience. Fork for one of these reasons:

- A reproducible failure in the lab.
- A performance problem tied to a concrete workload.
- Missing observability needed for safe operation.
- Packaging mismatch on the target TrueNAS release.

## Configuration Before Fork

Initial export policies should be expressed as explicit daemon arguments.

For repository datasets:

- `cache=auto` first, with `cache=none` as the strict-coherency fallback.
- `no_writeback` until writeback is proven safe with SMB-side edits.
- `xattr=on`.
- POSIX and flock lock support enabled.
- High open-file limit.
- Structured debug logging available but off by default.

For read-only sample datasets:

- host path exported read-only through policy and node publish mode.
- more aggressive cache policy can be tested.
- update flow should be explicit: SMB publish, service rescan, optional remount.

## Known Risk: File Descriptor Pressure

Large trees and metadata-heavy tools can drive high `virtiofsd` file descriptor
usage. This is relevant to git repos and package manager installs.

Mitigations to test before patching:

- Raise `virtiofsd` open file limits.
- Use `cache=none` or tune metadata caching.
- Reduce long-lived handle retention if exposed by daemon options.
- Track FD count as a first-class host-agent metric.

Fork patch candidates:

- bounded handle cache mode;
- better handle eviction logging;
- per-export FD watermark warnings;
- control socket endpoint that exposes open handle counts.

## Known Risk: Cache Coherency With SMB

SMB clients and VM workloads are touching the same host filesystem through
different stacks. Virtiofs cache settings decide how quickly the guest notices
host-side changes.

Fork patch candidates:

- expose cache invalidation statistics;
- add a host-agent control command to invalidate an export if upstream supports
  the necessary hooks;
- reduce or make deterministic metadata timeout behavior for this workload.

The v1 system should prefer policy and remount/reload workflows over clever
cache invalidation.

## Known Risk: Locks, ACLs, And xattrs

The repo dataset may need Linux tools, Samba ACLs, and extended attributes to
coexist. The sample dataset is simpler because Kubernetes mounts it read-only.

Lab first:

- Linux guest creates, renames, chmods, and deletes files.
- SMB client edits and renames files.
- Git lock files behave normally.
- Package manager lock files behave normally.
- ACLs and ownership look acceptable from SMB and the guest.

Fork only if a failing behavior is inside `virtiofsd-rs` rather than ZFS, Samba,
or mount policy.

## Known Risk: Daemon Death

If `virtiofsd` dies, the guest mount can hang or return I/O errors depending on
QEMU and guest behavior. The host agent should treat daemon death as a serious
node health event.

Host-agent behavior:

- mark export unhealthy immediately;
- report degraded status to CSI;
- optionally taint or cordon the node through Kubernetes integration later;
- restart the daemon only if lab proves QEMU reconnect behavior is safe;
- otherwise require node VM restart for a clean transport reset.

## Patch Discipline

If we fork:

- pin upstream commit;
- keep patches in a named branch;
- document every patch against a lab failure;
- package a single binary for the target TrueNAS host;
- avoid broad feature work that does not serve this deployment.

Sources:

- <https://virtio-fs.gitlab.io/>
- <https://gitlab.com/virtio-fs/virtiofsd>
- <https://virtio-fs.gitlab.io/qemu/tools/virtiofsd.html>
- <https://gitlab.com/virtio-fs/virtiofsd/-/work_items/121>
- <https://docs.redhat.com/en/documentation/red_hat_enterprise_linux/10/html/10.1_release_notes/known-issues>
