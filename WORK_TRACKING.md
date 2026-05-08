# Deployment Tracking

This is the active root-level tracker for getting `nas-csi` from the current
repo state to a usable deployment on one TrueNAS host. It is temporary progress
tracking, not durable architecture documentation; keep durable design material
under `docs/`.

The priority is to harden the smallest system that can safely serve the two
real workloads:

- SMB-visible git repository datasets used from Kubernetes for small-file build
  workloads.
- SMB-managed read-only VST/Kontakt content served by a Kubernetes workload.

Do not expand this tracker into broad platform work without an explicit
decision. New hardening should be tied to a deploy blocker, a reproducible
target-host failure, or a concrete dataset safety risk.

## Scope Rules

1. [ ] Keep the initial deployment to one TrueNAS host and IaC-owned VM-based
   k3s.
2. [ ] Keep host-specific facts, generated configs, secrets, image checksums,
   tokens, and kubeconfigs out of the repo.
3. [ ] Make static existing-dataset CSI the first storage deployment target.
4. [ ] Treat dynamic dataset provisioning as a later controller milestone, not a
   first-deploy requirement.
5. [ ] Do not fork or patch `virtiofsd` until a lab failure is reproducible on
   the target host.
6. [ ] Do not add HA or multi-host behavior beyond the maintenance use case of
   rebuilding/updating one VM node at a time.

## 1. First Deploy Packaging

Goal: produce the smallest set of artifacts needed to run the existing host
agent, cluster substrate, controller, and node plugin on the lab host.

1. [ ] Add container build definitions for `nas-csi-controller`.
2. [ ] Add container build definitions for `nas-csi-node`.
3. [ ] Ensure the node image contains required runtime tools such as `mount`
   and `umount`.
4. [ ] Add a simple image build/publish path, preferably through `xtask` or a
   short documented command sequence.
5. [ ] Pin the lab image tags used by `deploy/kubernetes/nas-csi/nas-csi.yaml`.
6. [ ] Keep the host-agent package path focused on the host binary, systemd
   assets, and Kubernetes manifests.

## 2. TrueNAS Host Bring-Up

Goal: prove host-agent-owned VM/runtime reconciliation on the actual TrueNAS
machine before any real application workloads depend on it.

1. [ ] Build the host-agent package.
2. [ ] Install the host-agent package on the TrueNAS host.
3. [ ] Run discovery on the TrueNAS host.
4. [ ] Materialize host-local config from discovery plus local selections.
5. [ ] Run `apply` dry-run and review applies, skips, and refusals.
6. [ ] Run guarded `apply --execute` for the first VM and selected virtiofs
   exports.
7. [ ] Verify root disk creation or reuse behavior.
8. [ ] Verify cloud-init seed image content and idempotence.
9. [ ] Verify libvirt domain existence, metadata ownership, and autostart
   behavior.
10. [ ] Verify `virtiofsd` systemd units are installed, enabled as expected, and
   running.
11. [ ] Verify virtiofs sockets become ready after service start or restart.
12. [ ] Verify the VM boots and the qemu guest agent responds.
13. [ ] Reboot TrueNAS and confirm `status` and `health` report the expected
   persistent state.
14. [ ] Confirm no selected TrueNAS datasets were mutated by the bring-up path.

## 3. k3s Cluster Bring-Up

Goal: prove the host agent can create and reconcile the k3s substrate on the
owned VM substrate without installing user workloads.

1. [ ] Run `cluster plan` against the materialized host config.
2. [ ] Run `cluster apply --execute` for the first server.
3. [ ] Verify k3s token creation and permissions.
4. [ ] Verify kubeconfig retrieval, rewrite, storage path, and permissions.
5. [ ] Verify the configured cluster API endpoint is reachable.
6. [ ] Add any additional node VMs and verify join ordering.
7. [ ] Verify expected nodes become Ready.
8. [ ] Verify configured labels and taints are reconciled.
9. [ ] Apply only substrate manifests: metrics server and `nas-csi`.
10. [ ] Reboot one VM node and confirm it returns cleanly.
11. [ ] Reboot the TrueNAS host and confirm cluster recovery behavior is
   understood.

## 4. Static Existing-Dataset CSI

Goal: mount existing TrueNAS filesystem datasets into pods while keeping those
datasets normal TrueNAS datasets and SMB shares.

1. [ ] Generate or install `/etc/nas-csi/node.yaml` on each VM from the
   host-local config.
2. [ ] Deploy `nas-csi` Kubernetes manifests with the lab image tags.
3. [ ] Create a static PV/PVC for the repository dataset.
4. [ ] Create a static PV/PVC for the read-only VST/Kontakt content dataset.
5. [ ] Verify `NodeStageVolume` uses the expected virtiofs-mounted source path.
6. [ ] Verify `NodePublishVolume` bind mounts into pod target paths.
7. [ ] Verify pod restart and node-plugin restart behavior.
8. [ ] Verify missing virtiofs exports fail closed.
9. [ ] Verify read-only policy prevents Kubernetes-side mutation of the content
   dataset.
10. [ ] Confirm SMB clients and Kubernetes pods see the same expected files.

## 5. Controller Backend

Goal: make the controller durable enough for real operation without making
dynamic provisioning part of the first deploy.

1. [ ] Add controller config loading instead of starting with default empty
   config.
2. [ ] Implement production TrueNAS API transport with API-key authentication,
   timeouts, and bounded retry behavior.
3. [ ] Choose one durable metadata model for controller-owned volume identity.
4. [ ] Wire existing-dataset registration to TrueNAS dataset and SMB metadata
   lookups.
5. [ ] Make controller restart behavior preserve volume identity.
6. [ ] Add fake TrueNAS API state-machine tests for registration, snapshots,
   SMB metadata, and delete safety.
7. [ ] Keep dynamic dataset creation disabled until existing-dataset behavior is
   verified on the lab host.
8. [ ] Add dynamic dataset create/delete only with explicit opt-in and explicit
   non-authoritative defaults.

## 6. Real Workload Validation

Goal: test the actual performance and coherency requirements that make this
project worth building.

1. [ ] Run repository workloads from a pod: `git status`, dependency install,
   clean build, and repeated small-file operations.
2. [ ] Edit repository files over SMB while observing the same tree from the VM
   and pods.
3. [ ] Record the chosen virtiofs cache policy and observed SMB/guest coherency
   behavior.
4. [ ] Run the VST/Kontakt read-only streaming server from a pod.
5. [ ] Manage VST/Kontakt files over SMB and verify pod-side visibility.
6. [ ] Capture `virtiofsd` CPU, memory, restart, and failure behavior during the
   real workloads.
7. [ ] Decide whether a `virtiofsd` or `virtiofsd-rs` fork is necessary only
   from a specific failing test case.

## 7. Minimal Observability

Goal: provide enough logs and status output to understand successful operations
and failures without adding a full monitoring stack.

1. [ ] Keep host-agent structured JSON logs for reconcile decisions and command
   execution in stderr/systemd journal.
2. [ ] Add structured logs for cluster reconcile operations, including positive
   actions, skips, refusals, and command failures.
3. [ ] Add startup logs for the CSI controller and node plugin with driver name,
   endpoint, config path, and mode.
4. [ ] Add CSI controller operation logs for create, delete, validate,
   publish/unpublish, list, and snapshot calls.
5. [ ] Add CSI node operation logs for stage, publish, unpublish, and unstage,
   including volume handle, export id, target path, read-only flag, and result.
6. [ ] Ensure failure logs include the same operation identifiers as success
   logs so a failed mount can be traced from Kubernetes event to pod logs.
7. [ ] Ensure logs never include API keys, kubeconfigs, cluster tokens, file
   contents, or full command output unless explicitly requested in a debug mode.
8. [ ] Document the first-deploy log commands: `journalctl` for host-agent and
   `kubectl logs` for controller/node pods.
9. [ ] Use metrics-server and `kubectl top` for basic CPU/memory visibility;
   do not add Prometheus, OpenTelemetry, or alerting until there is a concrete
   need.

## 8. Minimal Operations

Goal: have enough operational clarity to deploy and back out on the single
machine without inventing a general-purpose platform.

1. [ ] Write a first-deploy runbook using the exact commands that pass on the
   target host.
2. [ ] Write a rollback runbook for removing substrate without touching
   authoritative datasets.
3. [ ] Write a VM rebuild runbook for the maintenance use case.
4. [ ] Confirm TrueNAS snapshot, replication, quota, and retention tooling still
   sees the datasets normally.
5. [ ] Confirm SMB share definitions and permissions remain TrueNAS-owned.
6. [ ] Document the current deployment constraint: static existing datasets are
   supported first; dynamic provisioning waits for controller backend
   completion.

## Explicit Non-Goals For This Tracker

- Multi-host Kubernetes HA.
- Replacing SMB access.
- Moving authoritative dataset ownership into Kubernetes.
- UI registration as a deployment blocker.
- Dynamic PVCs as the default first-deploy path.
- Controller-managed retention or replication policy.
- A `virtiofsd` fork before there is a reproducible target-host failure.
- A broad hardening framework unrelated to the first deploy.

## Completed Foundation

The initial build-out already added the Rust workspace, typed config model,
read-only discovery, host-local materialization, VM artifact rendering,
cloud-init seed generation, state-aware host reconciliation, host-agent
packaging, k3s substrate reconciliation hooks, generated CSI bindings, CSI
controller and node services, Kubernetes substrate manifests, and local checks.

Those pieces are necessary but not sufficient for deployment. Items above should
only be marked complete when they are implemented and, where relevant, verified
against the target TrueNAS host.
