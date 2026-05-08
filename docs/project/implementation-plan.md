# Implementation Plan

This document is a durable roadmap. It is not a mirror of the Rust code or a
temporary progress checklist. The code and tests are the source of truth for
exact APIs and behavior.

## Current Implemented Baseline

The repository currently has a working Rust workspace with the main ownership
boundaries in place:

- repo-safe intent and local materialization model;
- read-only discovery;
- host VM/runtime artifact rendering;
- state-aware host reconciliation with guarded execution;
- host-agent packaging and systemd install assets;
- k3s cluster reconciliation planning and execution hooks;
- generated CSI protobuf bindings;
- CSI controller and node service implementations;
- static Kubernetes substrate manifests;
- example manifests and fictional config fixtures.

This is still pre-lab software. The next major risk is not type-checking; it is
proving the system against the target TrueNAS SCALE host, libvirt/QEMU versions,
guest image, and real datasets.

## Milestone 1: TrueNAS Host Lab Bring-Up

Goal: prove host-agent-owned VMs and runtime substrate on the actual host.

Work:

- install the host-agent package on TrueNAS;
- create local `/etc/nas-csi/host.yaml` through discovery and selections;
- validate base image checksum and qemu image info;
- run host `apply` dry-run and then guarded `apply --execute`;
- verify libvirt domains, autostart metadata, seed images, and virtiofsd units;
- reboot TrueNAS and confirm status returns to expected state.

Exit criteria:

- the host agent can recreate VM/runtime substrate from desired state;
- root disks remain disposable;
- selected TrueNAS datasets are not mutated except by explicit operator action;
- health output is useful for diagnosing missing tools, sockets, domains, and
  mounted datasets.

## Milestone 2: Virtiofs Transport Validation

Goal: prove the same SMB-visible datasets behave correctly through virtiofs.

Work:

- mount one read-write repository dataset in a node VM;
- mount one read-only sample/library dataset in a node VM;
- run repository workloads such as `git status`, install, and clean build;
- test SMB-side edits while the VM observes the same tree;
- verify read-only policy prevents Kubernetes-side mutation;
- capture `virtiofsd` resource use and failure modes.

Exit criteria:

- no unexplained guest hangs or `virtiofsd` crashes;
- SMB and guest visibility behavior is understood for chosen cache policy;
- repository and streaming workloads meet the user's practical performance bar;
- any need for a `virtiofsd-rs` fork is tied to a reproducible lab failure.

## Milestone 3: k3s Substrate Bring-Up

Goal: prove host-agent-owned cluster creation on the VM substrate.

Work:

- run `cluster plan` against local `HostConfig`;
- run `cluster apply --execute`;
- verify token and kubeconfig permissions;
- verify first-server bootstrap and join-node ordering;
- verify labels, taints, and node readiness;
- apply metrics-server and `nas-csi` substrate manifests;
- reboot nodes and confirm the cluster returns.

Exit criteria:

- kubeconfig points at the configured endpoint;
- all expected nodes become Ready;
- substrate manifests are applied and idempotent;
- no application workloads are installed by the host agent.

## Milestone 4: CSI Static Dataset Validation

Goal: mount existing TrueNAS datasets into pods through CSI.

Work:

- build and publish controller/node images for the lab;
- deploy `nas-csi` substrate manifests;
- create a static PV/PVC for an existing read-write dataset;
- create a static PV/PVC for an existing read-only dataset;
- exercise pod restart and node-plugin restart;
- verify missing virtiofs exports fail closed.

Exit criteria:

- pods see the expected dataset contents;
- pod target bind mounts are idempotent;
- read-only policy is enforced;
- node plugin restart does not leak mounts or hide failures.

## Milestone 5: Controller Backend Hardening

Goal: replace in-memory controller state with durable TrueNAS-backed behavior.

Work:

- implement production TrueNAS transport with authentication and retry policy;
- choose a durable metadata location for driver-owned volume state;
- wire `CreateVolume` existing-dataset registration to discovery/API lookups;
- wire optional dynamic dataset creation to TrueNAS API calls;
- implement authoritative delete safety with explicit opt-in;
- map SMB share metadata and snapshot metadata from TrueNAS records;
- add integration tests with a fake TrueNAS API state machine.

Exit criteria:

- controller restart does not lose volume identity;
- existing datasets are never deleted by default;
- dynamic dataset behavior is explicit and auditable;
- snapshot and SMB metadata match TrueNAS state.

## Milestone 6: Maintenance Workflows

Goal: make multi-node useful for planned updates.

Work:

- design drain/uncordon commands around Kubernetes API state;
- rebuild one agent VM without touching shared datasets;
- document `maintenance-basic` server outage behavior;
- add quorum checks for `maintenance-control-plane`;
- test k3s patch upgrades in ordered server/agent sequence;
- test cluster rebuild while preserving TrueNAS application datasets.

Exit criteria:

- one agent VM can be rebuilt while eligible workloads move elsewhere;
- server maintenance behavior is explicit for both profiles;
- cluster destroy/rebuild leaves authoritative datasets untouched.

## Milestone 7: Packaging And Release

Goal: make the system reproducible outside the development checkout.

Work:

- build controller and node images;
- pin image tags and sidecar versions;
- package host-agent deploy manifests;
- document install, rollback, and upgrade paths;
- add lab smoke tests to release validation;
- decide whether a pinned `virtiofsd-rs` binary is needed.

Exit criteria:

- a fresh host can be installed from release artifacts and local selections;
- release artifacts do not contain host-specific facts or secrets;
- rollback keeps TrueNAS application datasets intact.
