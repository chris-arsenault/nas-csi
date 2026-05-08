# crates/xtask

Developer automation crate.

Current commands:

```sh
cargo run -p nas-csi-xtask -- check
cargo run -p nas-csi-xtask -- package-host-agent
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

Planned commands:

- Generate CSI protobuf bindings.
- Generate host-agent protobuf bindings.
- Build release binaries.
- Build container images.
- Run lab smoke tests.
- Build or package a pinned `virtiofsd-rs` binary.
