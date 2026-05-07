# crates/node-plugin

CSI node service running inside each Kubernetes node VM.

Responsibilities:

- Implement CSI Node service.
- Read `/etc/nas-csi/node.yaml` and validate the VM/export contract generated
  by the host agent.
- Bind mounted virtiofs exports to CSI staging paths.
- Bind-mount staged paths into pod targets.
- Enforce read-only publishes.
- Clean up idempotently after pod and node plugin restarts.

Important rules:

- Never create a fake source directory if the virtiofs tag is absent.
- Verify mount type with `findmnt` or mountinfo.
- Treat missing host-agent export as a hard error.
- Keep all mount paths under kubelet and `/var/lib/nas-csi`.
