# Workload Validation

This runbook validates the real dataset behaviors that justify the project:
repository builds against an SMB-visible read-write dataset and read-only
streaming against an SMB-managed content dataset.

Run this only after `host-install`, `cluster install`, and `csi install` have
completed on the target TrueNAS host.

## Dry Run

Render the validation manifest and report scaffold:

```sh
nas-csi-host-agent workload validate \
  --config /etc/nas-csi/host.yaml
```

The command discovers the first read-write export for repository validation and
the first read-only export for content validation. Override that selection when
needed:

```sh
nas-csi-host-agent workload validate \
  --config /etc/nas-csi/host.yaml \
  --repo-export repos \
  --content-export samples
```

## Execute

Run the validation on the target host:

```sh
nas-csi-host-agent workload validate \
  --config /etc/nas-csi/host.yaml \
  --execute
```

The execute path:

- applies one repository pod and one read-only content pod;
- runs `git status`, dependency install/build from a copied npm project, and
  repeated small-file operations from the repository pod;
- runs a read-only streaming probe from the content pod;
- writes temporary `.nas-csi-validation` sentinel files into the selected
  TrueNAS dataset paths, verifies them through VM guest mounts and pods, and
  removes them;
- records `virtiofsd` systemd CPU, memory, restart, and status properties,
  libvirt cache policy, and guest mountinfo;
- restarts the repository export `virtiofsd` service and verifies workload pods
  can still read their mounts;
- writes the report under
  `/var/lib/nas-csi/rendered/workload-validation/report.txt`.

The validator does not create or delete TrueNAS datasets, change SMB shares, or
install application workloads. Validation pods are deleted on success unless
`--keep-pods` is used.

## Content Server

The default content probe uses BusyBox `httpd` over the read-only content mount.
To run the actual VST/Kontakt streaming server image, pass the image and command:

```sh
nas-csi-host-agent workload validate \
  --config /etc/nas-csi/host.yaml \
  --content-image registry.example.test/kontakt-streamer:lab \
  --content-command '/app/server --root /content --listen 0.0.0.0:8080' \
  --execute
```

Keep the server mounted read-only from Kubernetes. Manage content through the
normal TrueNAS/SMB path and rerun the validator to confirm pod-side visibility.

## Fork Decision

Do not fork `virtiofsd` or `virtiofsd-rs` from suspicion. Use the report to tie
any fork decision to a specific failing validation step: a reproducible
coherency error, crash, restart failure, hang, or unacceptable workload result.
