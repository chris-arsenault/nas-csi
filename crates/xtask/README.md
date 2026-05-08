# crates/xtask

Developer automation crate.

Current commands:

```sh
cargo run -p nas-csi-xtask -- check
cargo run -p nas-csi-xtask -- package-host-agent
cargo run -p nas-csi-xtask -- build-images
cargo run -p nas-csi-xtask -- push-images
```

`check` runs Rust formatting, workspace type checks, workspace tests, and
example YAML validation.

`package-host-agent` builds the release host-agent binary and creates
`dist/host-agent` with:

- `bin/nas-csi-host-agent`
- `nas-csi-host-agent.service`
- `nas-csi-host-agent.env`
- `install.sh`
- `uninstall.sh`
- `README.md`
- `deploy/addons`
- `deploy/kubernetes`

`build-images` builds the controller and node-plugin images using the default
lab tags referenced by `deploy/kubernetes/nas-csi/nas-csi.yaml`:

- `ghcr.io/chris-arsenault/nas-csi-controller:0.1.0-lab1`
- `ghcr.io/chris-arsenault/nas-csi-node:0.1.0-lab1`

`push-images` pushes those same tags. Both commands accept:

```sh
--runtime docker|podman
--registry REGISTRY
--tag TAG
```

Planned commands:

- Generate CSI protobuf bindings.
- Generate host-agent protobuf bindings.
- Build release binaries.
- Run lab smoke tests.
- Build or package a pinned `virtiofsd-rs` binary.
