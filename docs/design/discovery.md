# Discovery And Onboarding

## Position

The repo should not bake in host facts.

Examples may describe intent, such as `maintenance-basic` or
`maintenance-control-plane`, but concrete values must be discovered or generated
on the target TrueNAS host.

Host-specific generated config belongs under `/etc/nas-csi` or another local
operator-selected path, not in git.

## Discovery Output

The first implementation target should be a read-only discovery command:

```bash
nas-csi-host-agent discover --output /etc/nas-csi/discovery.yaml
```

It should collect:

- TrueNAS version and local middleware availability;
- available pools;
- filesystem datasets and mountpoints;
- SMB shares and paths;
- candidate datasets for VM image cache and VM state;
- libvirt URI and capability summary;
- QEMU version;
- available firmware and machine types;
- bridges and physical NICs;
- default route and LAN addresses;
- CPU and memory capacity;
- existing project-owned libvirt domains;
- available cloud images or image cache state;
- installed `virsh`, QEMU, `qemu-img`, `virtiofsd`, `systemctl`, and `midclt`
  paths and versions where available;
- kernel support needed by virtiofs;
- candidate k3s install source or cached binary.

Discovery must not create datasets, define VMs, install k3s, or start
`virtiofsd`.

On TrueNAS SCALE, the first concrete discovery path is local `midclt call` for
read-only middleware methods such as `system.version`, `pool.dataset.query`, and
`sharing.smb.query`. If `midclt` is unavailable or fails, discovery falls back
to conservative `/mnt` scanning so the setup flow can still produce a draft with
explicit warnings.

## Generated Desired State

The setup command combines:

- a repo intent file;
- discovered host facts;
- operator choices when discovery finds multiple safe candidates;
- generated secrets such as the k3s token;
- generated IDs such as MAC addresses and VM domain names.

Example:

```bash
nas-csi-host-agent init \
  --intent examples/intents/maintenance-basic.yaml \
  --discovery /etc/nas-csi/discovery.yaml \
  --output /etc/nas-csi/host.yaml
```

The resulting `/etc/nas-csi/host.yaml` is the concrete desired state. It is local
machine state and should not be committed.

## What Can Be Discovered

These should be discovered automatically:

- TrueNAS API endpoint when running locally;
- pool list;
- dataset list and mountpoints;
- SMB share list;
- bridge list;
- host IP addresses;
- libvirt/QEMU capabilities;
- host command availability for `virsh`, `qemu-img`, `virtiofsd`, `systemctl`,
  and `midclt`;
- CPU and memory capacity;
- existing `nas-csi` state datasets;
- existing `nas-csi` node VMs;
- existing generated MAC addresses and domain names.

## What Requires Selection

Discovery can find candidates, but some choices are still intent:

- cluster profile;
- which pool should hold VM state if multiple pools exist;
- which datasets should be exposed as `repos-dev` and `samples-ro`;
- whether setup may create missing system datasets;
- whether setup may download or import a cloud image;
- public SSH keys for node access;
- API endpoint DNS name if LAN DNS is desired;
- whether to use `maintenance-basic` or `maintenance-control-plane`.

The implementation should support non-interactive flags for these choices, but
the repo should not contain real values.

## Setup Phases

`discover`
: Read-only inventory.

`init`
: Generate a draft from discovery and intent.

`materialize`
: Combine intent, discovery, and local selections into concrete host desired
state.

`plan`
: Show what would change on TrueNAS/libvirt/k3s without mutating.

`apply`
: Make approved changes.

`status`
: Report actual state against desired state.

## Safety Rules

- Never infer that an arbitrary dataset should be deleted.
- Never store API keys, k3s tokens, kubeconfigs, MAC addresses, LAN IPs, or real
  dataset names in repo examples.
- Prefer generated local config under `/etc/nas-csi`.
- Keep `examples/` schematic.
- Mark every generated file with a header saying it is host-local state.
