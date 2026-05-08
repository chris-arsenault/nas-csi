# deploy/images

Container image definitions for the deployable CSI binaries.

The default lab image coordinates are:

- `ghcr.io/chris-arsenault/nas-csi-controller:0.1.0-lab1`
- `ghcr.io/chris-arsenault/nas-csi-node:0.1.0-lab1`

Build both images from the repo root with:

```sh
cargo run -p nas-csi-xtask -- build-images
```

Push both images with:

```sh
cargo run -p nas-csi-xtask -- push-images
```

Override the container runtime, registry, or tag when needed:

```sh
cargo run -p nas-csi-xtask -- build-images \
  --runtime podman \
  --registry registry.lan/nas-csi \
  --tag lab-test
```

The node image intentionally includes `mount` and `umount`, because the CSI
node service performs bind mounts inside a privileged DaemonSet container.

The host-agent package does not include this directory. Host installation and
container image publication are separate deployment steps.
