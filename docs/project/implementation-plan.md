# Implementation Plan

## Phase 0: Pin The Target

Target assumptions:

- One TrueNAS SCALE host.
- One or more host-agent-managed Linux Kubernetes node VMs on that same host.
- A host-agent-managed k3s cluster on those VMs.
- Datasets are normal ZFS filesystem datasets under `/mnt/<pool>/...`.
- Some datasets are SMB shares.
- Kubernetes needs the same file tree, not a copied or block-backed tree.
- Rust is the implementation language unless a specific host diagnostic forces a
  small shell shim.

Deliverables:

- Capture TrueNAS version, kernel, QEMU, libvirt, and available `virtiofsd`.
- Confirm direct libvirt domain ownership works without TrueNAS UI VM creation.
- Define the discovery inventory shape.
- Define the generated local config shape.
- Keep repo examples limited to intent and profile.

## Phase 1: Discovery And Local Config Proof

Goal: discover host facts and generate local desired state without mutating
TrueNAS, libvirt, or Kubernetes.

Status: initial implementation started. `nas-csi-host-agent` can validate intent
files, run local read-only discovery, generate a `HostConfigDraft`, and print a
non-mutating plan.

Steps:

1. Parse a repo intent file from `examples/intents`.
2. Discover TrueNAS version, pools, filesystem datasets, SMB shares, and
   mountpoints.
3. Discover libvirt URI, bridges, QEMU version, firmware, machine types, CPU,
   memory, and existing project-owned domains.
4. Discover `virtiofsd` path and version.
5. Discover or select candidate image-cache and VM-state datasets.
6. Generate local `HostConfig` under a non-repo path.
7. Generate a `plan` output showing intended VMs, k3s cluster shape, exports,
   and required operator approvals.

Exit criteria:

- no host mutation occurs;
- examples remain free of host-specific facts;
- generated config contains all concrete host values;
- `plan` can explain missing prerequisites and safe next actions.

## Phase 2: Manual IaC VM Proof

Goal: prove that a non-UI libvirt node VM can be owned cleanly on the TrueNAS
host.

Steps:

1. Discover or create the selected TrueNAS dataset for `nas-csi` VM state.
2. Discover, import, or download a selected Linux cloud image.
3. Create a per-node root disk.
4. Generate a NoCloud seed image.
5. Define a libvirt domain with shared-memory backing.
6. Attach it to the discovered or selected LAN bridge.
7. Boot it without using the TrueNAS VM UI.
8. Verify serial console, SSH, qemu guest agent, reboot, shutdown, autostart,
   and domain persistence across host reboot.

Exit criteria:

- TrueNAS UI does not need to know about or mutate the domain.
- The domain survives middleware restart and host reboot.
- Shared-memory backing is present from the first boot.
- The node can be destroyed and rebuilt from desired state.

## Phase 3: Manual Transport Proof

Goal: prove the storage channel before writing CSI.

Steps:

1. Build or install a pinned Rust `virtiofsd`.
2. Configure the node VM with shared memory backing.
3. Attach one selected read-write virtiofs export.
4. Attach one selected read-only virtiofs export.
5. Mount both inside the VM.
6. Run git, npm, and streaming benchmarks.
7. Edit files over SMB and observe VM visibility.
8. Snapshot datasets in TrueNAS during VM reads and writes.

Exit criteria:

- No unexplained guest hangs.
- No `virtiofsd` crashes.
- File descriptor use is bounded or has a known tuning fix.
- SMB edits become visible under the selected cache policy.
- Read-only sample mount cannot mutate host files from Kubernetes.

## Phase 4: Manual k3s Cluster Proof

Goal: prove the same desired-state system can stand up the substrate cluster.

Steps:

1. Pin a k3s version.
2. Render `/etc/rancher/k3s/config.yaml` through cloud-init.
3. Bootstrap the first server node with `cluster-init: true`.
4. Retrieve `/etc/rancher/k3s/k3s.yaml`.
5. Join an agent node with the shared token.
6. Verify both nodes become Ready.
7. Install a placeholder system add-on through Helm or Kubernetes API.
8. Reboot nodes and verify the cluster returns.

Exit criteria:

- k3s install is repeatable from desired state.
- kubeconfig is captured and points at the configured endpoint.
- node labels and taints are reconciled.
- no application workloads are installed.
- the cluster can be destroyed without deleting TrueNAS app datasets.

## Phase 5: Rust Workspace Skeleton

Goal: make the component boundaries real.

Deliverables:

- Workspace `Cargo.toml`.
- `types` crate with volume policy structs.
- `truenas-client` crate with `system.info`, dataset query, SMB query, and VM
  query.
- `vm-manager` crate with domain XML rendering, cloud-init seed generation,
  root disk image planning, and state-aware host reconciliation planning.
- `cluster-manager` crate with k3s config rendering, bootstrap planning, token
  management, kubeconfig handling, and add-on reconciliation.
- gRPC protobuf for host-agent API.
- CSI protobuf generation strategy.

Rust CSI decision:

- Do not depend directly on the stale `k8s-csi` crate for production.
- Use it as a reference if useful.
- Generate CSI bindings from the upstream CSI spec with current `tonic` and
  `prost` through `xtask`.

## Phase 6: Host Agent MVP

Goal: automate the manual VM, cluster, and transport proof.

Deliverables:

- `nas-csi-host-agent` systemd service.
- Static config file listing node VM, k3s cluster, and dataset export desired
  state.
- VM create/start/stop/destroy/rebuild commands.
- root disk and cloud-init seed reconciliation.
- k3s cluster bootstrap and status commands.
- `EnsureExport` and `GetExportHealth` gRPC methods.
- `virtiofsd` supervisor with pinned arguments per policy.
- Socket directory management under `/run/nas-csi`.
- Libvirt/QEMU reconciliation for node domains and VM filesystem devices.
- Structured logs.

Exit criteria:

- Reboot TrueNAS, start the agent, and get the same VM and mounts back.
- Reboot TrueNAS and get the same k3s cluster back.
- Restart the node VM and get the same virtiofs devices back.
- Kill `virtiofsd` and observe clear unhealthy state.
- Change desired config and reconcile without stale sockets.

## Phase 7: Node Plugin MVP

Goal: mount a host-agent export into a pod through CSI.

Deliverables:

- CSI Identity service.
- CSI Node service:
  - `NodeGetInfo`
  - `NodeStageVolume`
  - `NodePublishVolume`
  - `NodeUnpublishVolume`
  - `NodeUnstageVolume`
- Mount wrapper using `findmnt` and `/proc/self/mountinfo`.
- Strict checks that prevent empty-directory shadow mounts.
- Static PV example for a selected read-write export.
- Static PV example for a selected read-only export.

Exit criteria:

- A pod sees the selected read-write dataset.
- A pod sees the selected read-only dataset read-only.
- Pod restart does not leak mounts.
- Node plugin restart is idempotent.

## Phase 8: Controller MVP

Goal: make static existing datasets feel native to Kubernetes.

Deliverables:

- CSI Controller service:
  - `ValidateVolumeCapabilities`
  - `ControllerPublishVolume`
  - `ControllerUnpublishVolume`
- `CreateVolume` for pre-existing dataset registration.
- StorageClass parameters for:
  - dataset name
  - subpath
  - policy
  - read-only default
  - destructive delete protection
- Calls to host-agent `EnsureExport`.

Exit criteria:

- PVC can bind to an existing dataset or subpath.
- Scheduling onto the node triggers host-agent export reconciliation.
- Repeated publish/unpublish calls are harmless.

## Phase 9: Snapshots And Clones

Goal: integrate with TrueNAS data protection without replacing it.

Deliverables:

- `CreateSnapshot` and `DeleteSnapshot`.
- Snapshot naming policy compatible with existing retention tasks.
- Optional `CreateVolume` from snapshot as a new cloned dataset.
- Documentation that TrueNAS periodic snapshot tasks remain authoritative for
  normal retention.

Exit criteria:

- Kubernetes VolumeSnapshot creates a ZFS snapshot.
- Deleting the Kubernetes snapshot respects holds and retention safety.
- Clone produces a separate TrueNAS filesystem dataset.

## Phase 10: Hardening

Goal: make it reliable for your actual workloads.

Work items:

- Cache policy matrix for selected read-write and read-only policies.
- SMB coherency tests.
- Lock behavior tests between Samba and Linux guest processes.
- Large directory traversal tests for FD pressure.
- `npm install`, `pnpm install`, and clean build benchmarks.
- VST/Kontakt streaming latency and throughput benchmarks.
- Host-agent metrics endpoint.
- Cluster backup and restore drills.
- Ordered k3s upgrade drills.
- Node cordon or taint integration when exports are unhealthy.
- Backup and restore drills using TrueNAS snapshots.

## Phase 11: Maintenance Workflows

Goal: make multi-node useful for planned node updates.

Work items:

- `drain-node` and `uncordon-node` commands.
- agent node rebuild with workloads rescheduled elsewhere.
- server node restart in `maintenance-basic` with documented API outage.
- server node rolling restart in `maintenance-control-plane` with quorum checks.
- VM root image update workflow.
- k3s patch upgrade workflow.
- preflight checks for replacement capacity and disruption policy.

Exit criteria:

- one agent VM can be rebuilt without stopping replicated workloads;
- server maintenance behavior is explicit for both profiles;
- cluster destroy/rebuild leaves TrueNAS app datasets untouched.

## Phase 12: virtiofsd Fork If Needed

Fork only after Phase 3 or Phase 10 identifies a concrete defect.

Likely patch areas:

- Better FD reclamation or bounded handle cache behavior for repository trees.
- ZFS-backed export instrumentation.
- Health/control socket for host-agent.
- Deterministic cache mode defaults.
- Improved logging around stuck requests.
- Packaging a known-good daemon for the exact TrueNAS release.

Exit criteria:

- Patch is tied to a failing lab test.
- Forked binary is pinned and packaged by `xtask`.
- Upstream issue or patch is linked where appropriate.
