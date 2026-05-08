# crates/vm-manager

Libvirt and node VM lifecycle library used by `nas-csi-host-agent`.

Responsibilities:

- Render libvirt domain XML from typed desired state.
- Validate host libvirt capabilities.
- Define, start, shutdown, destroy, undefine, and autostart domains.
- Manage node root disk images.
- Generate cloud-init NoCloud seed images.
- Render virtiofs filesystem devices using agent-owned sockets.
- Store `nas-csi` metadata in domain XML.

This crate should be testable without a live TrueNAS host by comparing generated
XML and planned operations against fixtures.

Rendered artifact layout:

```text
nodes/<node>/domain.xml
nodes/<node>/cloud-init/user-data
nodes/<node>/cloud-init/meta-data
nodes/<node>/k3s/config.yaml
nodes/<node>/systemd/nascsi-virtiofsd-<domain>-<export>.service
```

The generated domain XML assumes QEMU shared memory backing for virtiofs. The
generated systemd units keep virtiofsd under host-agent ownership by using
stable sockets under `/run/nas-csi/virtiofs`.

Domain XML includes `nas-csi` metadata plus a hash of the project-owned domain
shape. Reconciliation refuses existing unmarked domains unless adoption is
explicitly enabled, and compares the managed marker instead of raw `virsh
dumpxml` output because libvirt expands domain XML after define.

Cloud-init also writes `/etc/nas-csi/node.yaml` and a managed virtiofs fstab
fragment so each VM mounts exports under `/var/lib/nas-csi/virtiofs/<export>`.
The CSI node plugin uses those mounted paths as its bind-mount source of truth.

Apply planning is typed rather than shell-based. The executor surface plans:

- idempotent artifact and systemd unit writes;
- root disk qcow2 overlay creation guarded by `creates`;
- cloud-init NoCloud seed image generation using an internal Rust VFAT writer;
- `systemctl daemon-reload` and `systemctl enable --now` for virtiofsd units;
- `virsh define` and `virsh autostart` for node domains.

Seed images are small VFAT volumes labeled `CIDATA` with `user-data` and
`meta-data` files at the root.

Reconcile planning consumes actual host state and converts desired state into
named operations such as `CreateRootDisk`, `ResizeRootDisk`, `RewriteSeedImage`,
`InstallOrUpdateSystemdUnit`, `RestartVirtiofsdService`, `DefineDomain`, and
`RedefineDomainRequiresShutdown`. It skips matching files and seed images by
content hash, validates base image existence, format, and SHA-256 before root
disk creation, validates existing root disk overlays with `qemu-img info`,
refuses unknown or mismatched root disks instead of replacing them, reloads or
restarts systemd units only when their installed contents change, and refuses to
redefine a running libvirt domain unless explicitly allowed.
