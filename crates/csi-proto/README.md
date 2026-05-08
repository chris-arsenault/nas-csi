# crates/csi-proto

Generated CSI protobuf bindings used by the controller and node plugin crates.

The crate builds `proto/csi.proto` with `tonic-prost-build` and a vendored
`protoc`, so local development does not require a system `protoc` package.
Generated Rust is not committed.
