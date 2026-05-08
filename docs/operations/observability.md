# Observability

`nas-csi` uses plain logs and Kubernetes metrics-server for the first deploy.
There is no Prometheus, OpenTelemetry, alerting, or external telemetry stack in
the minimal deployment.

## Host Logs

The host agent writes structured JSON logs to stderr. Under systemd, read them
from the journal:

```sh
journalctl -u nas-csi-host-agent --since today
journalctl -u nas-csi-host-agent -f
```

For virtiofs services managed by the host agent:

```sh
journalctl -u 'nascsi-virtiofsd-*' --since today
journalctl -u 'nascsi-virtiofsd-*' -f
```

Host-agent command logs redact qemu guest-agent payloads, kubeconfig/token-style
flag values, heredoc file-content markers, and oversized argv values. They are
intended to show what operation ran and whether it succeeded, not to preserve
command output or file contents.

## Kubernetes Logs

Controller logs:

```sh
kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  -n kube-system logs deployment/nas-csi-controller -c nas-csi-controller
```

Node plugin logs:

```sh
kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  -n kube-system logs daemonset/nas-csi-node -c nas-csi-node
```

Follow logs during a mount test:

```sh
kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  -n kube-system logs deployment/nas-csi-controller -c nas-csi-controller -f

kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  -n kube-system logs daemonset/nas-csi-node -c nas-csi-node -f
```

The CSI controller logs startup context and controller RPC outcomes. The node
plugin logs startup context and node RPC outcomes. Success and failure entries
carry the same identifiers, including volume id, export id, node id where
available, target path, staging path, and read-only state where CSI supplies it.

## Resource Visibility

The cluster installer applies metrics-server as substrate when enabled in
`HostConfig`. Use `kubectl top` for basic CPU and memory visibility:

```sh
kubectl --kubeconfig /etc/nas-csi/kubeconfig top nodes
kubectl --kubeconfig /etc/nas-csi/kubeconfig -n kube-system top pods
kubectl --kubeconfig /etc/nas-csi/kubeconfig -n kube-system top pod \
  -l app.kubernetes.io/name=nas-csi
```

For host-side `virtiofsd` CPU and memory during real workloads, use:

```sh
nas-csi-host-agent workload validate \
  --config /etc/nas-csi/host.yaml \
  --execute
```

The validation report records systemd `CPUUsageNSec`, `MemoryCurrent`, restart
state, service result, libvirt cache policy, and guest mountinfo for the selected
repository and content exports.

## Boundaries

Do not add a broader monitoring stack until a concrete operational need exists.
For the initial single-host deployment, logs, `health`, `status`,
`workload validate`, metrics-server, and `kubectl top` are the supported
observability surface.
