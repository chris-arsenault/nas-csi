# IaC Node VMs On Libvirt

## Research Finding

Direct libvirt ownership is the cleanest path for this project.

Virtiofs requires specific domain-level configuration. Libvirt's virtiofs guide
requires shared-memory backing and shows the guest mount tag model. It also
supports externally launched `virtiofsd` sockets, where the application running
the daemon owns the socket, mount tag, and daemon options. That lines up exactly
with `nas-csi-host-agent`.

Cloud-init NoCloud supports local seed media with the `CIDATA` label. That gives
us deterministic VM bootstrap without needing a metadata service on first boot.

The Rust `virt` crate provides libvirt bindings. It should be evaluated in the
lab, with `virsh` kept as a diagnostic fallback.

## Why This Is Easier Than Adopting TrueNAS UI VMs

With host-agent-owned VMs:

- memory backing is correct from domain creation;
- virtiofs PCI/device layout is part of the template;
- root disk and cloud-init are reproducible;
- no TrueNAS UI VM regeneration path can silently remove storage devices;
- nodes are disposable and rebuildable;
- storage datasets stay outside the VM lifecycle.

With adopted TrueNAS UI VMs:

- the agent has to detect and repair middleware-generated config changes;
- some changes require restarts anyway;
- command-line escape hatches may be version-specific;
- the VM UI becomes another state owner.

## Domain Requirements

The managed node domain should include:

- KVM/QEMU domain under `qemu:///system`;
- Q35 machine type;
- UEFI when available;
- host CPU model;
- virtio network on a known bridge;
- virtio root disk;
- serial console;
- qemu guest agent channel;
- shared-memory backing:

```xml
<memoryBacking>
  <source type='memfd'/>
  <access mode='shared'/>
</memoryBacking>
```

For each virtiofs export, use an externally launched socket:

```xml
<filesystem type='mount'>
  <driver type='virtiofs' queue='1024'/>
  <source socket='/run/nas-csi/virtiofs/<node>/<export>.sock'/>
  <target dir='<generated-virtiofs-tag>'/>
</filesystem>
```

## Open Lab Questions

- Does TrueNAS ship a libvirt daemon configuration that permits project-owned
  domains under `qemu:///system` without middleware cleanup?
- Where should persistent domain XML and NVRAM live across TrueNAS upgrades?
- Is the existing LAN bridge suitable for direct VM attachment?
- Is AppArmor/SELinux or libvirt security labeling active in a way that blocks
  `/mnt/<pool>` virtiofs exports?
- Is the installed QEMU/libvirt new enough for externally launched virtiofsd
  sockets and memfd memory backing?
- Does autostart run early enough relative to `nas-csi-host-agent` and
  `virtiofsd` sockets?

## Recommendation

Make host-agent-owned VMs the default. Keep adopted TrueNAS UI VMs as a later
compatibility experiment only if needed.

Sources:

- <https://libvirt.gitlab.io/libvirt/kbase/virtiofs.html>
- <https://www.libvirt.org/formatdomain.html>
- <https://libvirt.org/formatnetwork.html>
- <https://docs.cloud-init.io/en/latest/reference/datasources/nocloud.html>
- <https://docs.rs/virt/latest/virt/>
