# Rust CSI Implementation

## Binding Strategy

There is an existing `k8s-csi` Rust crate that generates CSI bindings with
`tonic` and `prost`, but its published version is old and based on CSI v1.3.0.
Use it as a reference, not as a production dependency.

Preferred path:

1. Vendor or fetch the upstream CSI proto at a pinned version.
2. Generate Rust bindings with current `tonic-build` and `prost-build`.
3. Wrap generated request/response types in project-owned service modules.
4. Keep CSI spec version explicit in `Cargo.toml` or `xtask` output.

## Controller Shape

The controller process should serve CSI over a Unix socket shared with standard
CSI sidecars.

Implemented services:

- Identity.
- Controller.

MVP calls:

- `GetPluginInfo`
- `GetPluginCapabilities`
- `Probe`
- `ValidateVolumeCapabilities`
- `CreateVolume` for existing dataset registration.
- `DeleteVolume` with destructive protection.
- `ControllerPublishVolume`
- `ControllerUnpublishVolume`

Later:

- snapshots;
- clones;
- quota updates;
- storage capacity reporting only if useful.

## Node Shape

The node plugin should serve CSI over a Unix socket registered with kubelet
through `node-driver-registrar`.

Implemented services:

- Identity.
- Node.

MVP calls:

- `NodeGetInfo`
- `NodeGetCapabilities`
- `NodeStageVolume`
- `NodePublishVolume`
- `NodeUnpublishVolume`
- `NodeUnstageVolume`

No raw block support is needed for same-dataset storage.

## Deployment Shape

Use standard CSI sidecars, not custom Kubernetes controllers until a real gap
appears.

Controller Deployment:

- `nas-csi-controller`
- `external-provisioner`
- optional `external-attacher`
- later `external-snapshotter`
- `livenessprobe`

Node DaemonSet:

- `nas-csi-node-plugin`
- `node-driver-registrar`
- `livenessprobe`

## Rust Crates

Likely dependencies:

- `tokio`
- `tonic`
- `prost`
- `serde`
- `serde_json`
- `tracing`
- `thiserror`
- `tokio-tungstenite`
- `nix` or `rustix` for mount-adjacent syscalls if needed

Shelling out to `mount` and `findmnt` is acceptable for the first node plugin
because Linux mount behavior and kubelet expectations are easier to validate
through system tools. Replace with direct syscalls only when the wrapper becomes
a real limitation.

Sources:

- <https://kubernetes-csi.github.io/docs/>
- <https://kubernetes-csi.github.io/docs/deploying.html>
- <https://github.com/container-storage-interface/spec>
- <https://docs.rs/crate/k8s-csi/latest>
