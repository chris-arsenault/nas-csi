# VM Management

## Position

`nas-csi-host-agent` should own the Kubernetes node VMs directly.

That is not scope creep for this project. It simplifies the storage problem
because virtiofs correctness depends on VM definition details:

- shared-memory backing;
- filesystem device model;
- PCI topology;
- mount tags;
- externally launched `virtiofsd` sockets;
- restart behavior when the transport changes.

If TrueNAS UI-created VMs own those details, the host agent has to fight
middleware regeneration. If the host agent owns the VM, the desired state is
plain IaC.

## Ownership Boundary

TrueNAS owns:

- ZFS pools and filesystem datasets;
- SMB shares;
- snapshots, replication, quotas, and retention;
- host boot and appliance services.

`nas-csi-host-agent` owns:

- Kubernetes node VM definitions;
- VM root disk images;
- cloud-init seed data;
- libvirt domain metadata;
- VM start, stop, restart, destroy, and rebuild operations;
- virtiofs device definitions;
- `virtiofsd` processes.

Kubernetes owns:

- workloads;
- CSI PV/PVC lifecycle;
- node plugin mounts inside the guest;
- optional k3s or Kubernetes cluster state after bootstrap.

## Desired State

The host agent should reconcile a declarative config file. A future controller
could generate this, but v1 can be explicit YAML.

The concrete `HostConfig` is generated locally by discovery and init. It should
contain discovered or generated values like:

```yaml
apiVersion: nas-csi.dev/v1alpha1
kind: HostConfig

truenas:
  url: <discovered-local-truenas-api-url>
  apiKeyFile: <local-api-key-file>

hostTools:
  virsh: <discovered-virsh-path>
  qemuImg: <discovered-qemu-img-path>
  virtiofsd: <discovered-virtiofsd-path>
  systemctl: <discovered-systemctl-path>

libvirt:
  uri: qemu:///system
  bridge: <discovered-bridge>

imageCache:
  dataset: <selected-image-cache-dataset>

vmState:
  dataset: <selected-vm-state-dataset>

nodes:
  - name: <generated-node-name>
    domain: <generated-domain-name>
    autostart: true
    vcpus: <derived-or-selected-vcpu-count>
    memoryMiB: <derived-or-selected-memory>
    machine: <discovered-machine-type>
    firmware: <discovered-firmware>
    cpu: <discovered-cpu-mode>
    network:
      bridge: <discovered-bridge>
      mac: <generated-mac-address>
    rootDisk:
      image: <generated-node-root-disk>
      sourceImage: <selected-cloud-image>
      sourceFormat: qcow2
      sourceChecksum: sha256:<selected-cloud-image-sha256>
      sizeGib: <derived-or-selected-size>
      format: qcow2
    cloudInit:
      hostname: <generated-hostname>
      sshAuthorizedKeys: <operator-provided-public-keys>
    exports:
      - <selected-export-id>

exports:
  <selected-export-id>:
    dataset: <selected-truenas-dataset>
    sourcePath: /mnt/<pool>/<dataset>
    tag: <generated-virtiofs-tag>
    policy: <selected-policy>
    access: read-write
```

## Domain Shape

The generated libvirt domain should be boring and reproducible:

- `qemu:///system`;
- Q35 machine type;
- UEFI firmware when available;
- `host-passthrough` or `host-model` CPU;
- virtio root disk;
- virtio network device on a known bridge;
- serial console and qemu guest agent channel;
- no graphics by default;
- autostart enabled;
- shared memory backing required for virtiofs.

Virtiofs requires shared guest memory. The domain should include:

```xml
<memoryBacking>
  <source type='memfd'/>
  <access mode='shared'/>
</memoryBacking>
```

For externally launched `virtiofsd`, the filesystem device should reference the
agent-owned socket:

```xml
<filesystem type='mount'>
  <driver type='virtiofs' queue='1024'/>
  <source socket='/run/nas-csi/virtiofs/<node>/<export>.sock'/>
  <target dir='<generated-virtiofs-tag>'/>
</filesystem>
```

The host agent starts `virtiofsd` with the mount tag and cache/lock/xattr policy
for that export. Libvirt then connects QEMU to the already managed socket.

## Root Disk Strategy

Node root disks are disposable. They should not contain authoritative app data.

Initial strategy:

- Store base images under the discovered or selected image-cache dataset.
- Store node root disks under the discovered or selected VM-state dataset.
- Use qcow2 for early velocity because cloud images are already qcow2 and
  overlays are convenient.
- Keep the option to switch root disks to raw files or zvols if benchmarked
  necessary.

The host agent should support:

- importing a user-provided base image;
- validating image checksum;
- creating a per-node copy or overlay;
- expanding the node root disk;
- destroying and rebuilding a node without touching shared datasets.

The first apply planner emits `qemu-img create` commands for per-node qcow2
overlays. The command is guarded by a `creates` path so rerunning apply does not
replace an existing node root disk.

## Cloud-Init Strategy

Use cloud-init NoCloud for Linux node bootstrap.

Preferred v1 implementation:

- Generate a small `CIDATA` vfat or iso9660 seed image per node.
- Attach it as a read-only CD-ROM or disk.
- Put `user-data`, `meta-data`, and optional `network-config` in the seed.

This avoids depending on a network metadata service during first boot.

The host agent does not depend on `genisoimage` or `cloud-localds`. It generates
a small VFAT seed image in Rust. The image is labeled `CIDATA` and contains the
NoCloud `user-data` and `meta-data` files at the filesystem root.

Bootstrap content:

- hostname;
- SSH keys;
- qemu guest agent;
- base packages;
- k3s install and config when cluster ownership is enabled;
- `/etc/nas-csi/node.yaml` with node identity and expected virtiofs tags.

## Apply Shape

`nas-csi-host-agent apply` defaults to dry-run. It renders the typed desired
plan, inspects host state, and prints a reconcile decision for each step. The
typed plan currently covers:

- writing rendered artifacts under a local artifact directory;
- creating root disk parent directories;
- creating missing qcow2 root overlays from `rootDisk.sourceImage`;
- regenerating disposable cloud-init seed images with the internal Rust VFAT
  writer;
- installing virtiofsd systemd units;
- running `systemctl daemon-reload` and `systemctl enable --now`;
- running `virsh define` and `virsh autostart`.

The reconciler skips files and seed images that already match by content hash,
validates base image existence, format, and SHA-256 before root disk creation,
validates existing root disk overlays with `qemu-img info`, plans safe root disk
growth with `qemu-img resize`, restarts active virtiofsd services when their
installed unit content changes, waits for their Unix sockets after start or
restart, and compares libvirt domains through `nas-csi` metadata instead of raw
`virsh dumpxml` bytes. Raw libvirt XML is not stable enough for equality because
libvirt expands definitions after `virsh define`.

The reconcile diff is intentionally typed. It uses named operations such as
`CreateRootDisk`, `RewriteSeedImage`, `InstallOrUpdateSystemdUnit`,
`RestartVirtiofsdService`, `DefineDomain`, and
`RedefineDomainRequiresShutdown`, instead of carrying opaque shell strings.
Host execution routes commands through `program + argv` specs. Existing root
disks are never replaced by apply; an unknown or mismatched existing image is a
refusal.

VM start is still excluded from the default CLI path, but the planner has a
state-aware `start_domains` option for the future execute policy. If enabled, it
skips domains that are already running and starts only stopped or missing ones
after the define/autostart steps are safe.

Existing libvirt domains must carry `nas-csi` metadata before reconcile will
manage them. An unmanaged domain with the same name is refused unless an
explicit adoption option is enabled, and stopped-domain redefines remain stopped
unless the separate start policy is enabled.

## K3s/Kubernetes Bootstrap

The preferred mode is `k3s-owned`: the host agent renders cloud-init that
installs k3s server/agent roles, manages the cluster token, retrieves kubeconfig,
and installs the `nas-csi` substrate components.

`vm-only`
: Compatibility mode where the host agent creates VMs and storage transport
  only. Kubernetes or k3s is installed by separate automation.

See [Cluster Management](cluster-management.md) for the cluster-level ownership
model.

## TrueNAS UI Visibility

Nice-to-have, not a requirement.

The clean VM ownership model means `nas-csi-host-agent` defines libvirt domains
directly. Based on the current TrueNAS API shape, those domains should not be
expected to automatically appear in the TrueNAS VM UI. The VM UI appears to be
backed by middleware VM records exposed through `vm.query`, `vm.create`,
`vm.update`, and related VM methods, not by a generic inventory of every libvirt
domain on the host.

There is no documented `vm.import` or `vm.adopt` method in the current API. So
"post-register this external libvirt domain into the UI" should be treated as an
unsupported experiment unless the target TrueNAS version proves otherwise.

### Preferred Nice-To-Have

Build a small host-agent UI and metrics surface instead of making TrueNAS the
manager of these VMs.

Host-agent should expose:

- VM state: running, stopped, PID, uptime, autostart policy;
- CPU, memory, network, disk, and virtiofs export health;
- serial console or SSH connection helper;
- qemu guest agent status;
- node rebuild/restart controls;
- links to Kubernetes node status and workloads.

This gives the useful operational surface without giving TrueNAS middleware
ownership of the domain XML.

### Possible TrueNAS Integration

A later experiment can try a read-only or low-risk UI integration:

- discover whether TrueNAS has an internal-only import/adopt path;
- test whether a middleware-created VM with `command_line_args` can carry the
  required virtiofs QEMU arguments without losing shared-memory backing;
- verify whether TrueNAS start/stop/restart preserves all required options;
- verify whether UI edits rewrite or drop the custom transport configuration.

This mode should not be the default. If TrueNAS middleware owns the VM record,
then the host agent no longer has exclusive control over the VM definition.

### What Not To Do

Do not insert rows directly into the TrueNAS middleware database. That creates an
unversioned dependency on appliance internals and can break on upgrade.

Do not create a fake TrueNAS VM record that points at a separately managed
libvirt domain unless the official API explicitly supports that ownership model.

## Reconciliation

The host agent reconciliation loop should be conservative:

1. Validate TrueNAS API readiness.
2. Validate desired datasets and bridge.
3. Ensure image and VM state datasets exist.
4. Ensure base images are present and checksummed.
5. Render desired libvirt domain XML.
6. Compare the desired `nas-csi` domain metadata hash with current domain
   metadata from `virsh dumpxml`.
7. Apply changes that are safe while stopped.
8. Refuse unsafe live changes unless explicitly requested.
9. Start or restart VMs according to policy.
10. Start and verify `virtiofsd` exports.
11. Report node/export health.

For v1, changing memory, CPU topology, root disk, or virtiofs devices should
require a planned VM restart. That is acceptable for a single-host system and
keeps the implementation honest.

## Destructive Operations

Destroying a node VM may delete:

- libvirt domain definition;
- node root disk;
- cloud-init seed;
- transient sockets;
- `virtiofsd` processes for that node.

It must never delete:

- SMB-visible application datasets selected for export;
- snapshots;
- replication tasks;
- TrueNAS shares unless explicitly requested by a separate storage operation.

## Implementation Notes

Rust options:

- Use the `virt` crate for libvirt bindings after lab validation.
- Keep a `virsh` fallback for early diagnostics and integration tests.
- Generate XML from typed structs and templates, then validate with libvirt.
- Store project metadata in the libvirt domain metadata namespace so the agent
  can identify domains it owns.

Avoid:

- hand-edited XML as state;
- TrueNAS UI VM creation for managed nodes;
- live hotplug as a v1 requirement;
- assuming VM snapshots protect virtiofs-shared data.

Sources:

- <https://libvirt.gitlab.io/libvirt/kbase/virtiofs.html>
- <https://www.libvirt.org/formatdomain.html>
- <https://libvirt.org/formatnetwork.html>
- <https://docs.cloud-init.io/en/latest/reference/datasources/nocloud.html>
- <https://docs.rs/virt/latest/virt/>
