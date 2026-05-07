# crates/xtask

Developer automation crate.

Current command:

```sh
cargo run -p nas-csi-xtask -- check
```

This runs Rust formatting, workspace type checks, workspace tests, and example
YAML validation.

Planned commands:

- Generate CSI protobuf bindings.
- Generate host-agent protobuf bindings.
- Build release binaries.
- Build container images.
- Package the host-agent systemd unit.
- Run lab smoke tests.
- Build or package a pinned `virtiofsd-rs` binary.
