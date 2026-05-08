# First Deploy

This runbook brings up the smallest supported deployment on one TrueNAS host:
host-agent-owned VMs, k3s substrate, static existing-dataset CSI, observability,
and workload validation. It does not deploy application workloads.

Commands assume the packaged host-agent is installed on the TrueNAS host and
that local intent and selections have been written under `/etc/nas-csi`.

## Preconditions

- TrueNAS datasets and SMB shares already exist.
- The selected base VM image is present on a TrueNAS dataset and has a known
  SHA-256 checksum.
- The selected bridge, VM state dataset, image cache dataset, and export
  datasets were discovered on the target host.
- `/etc/nas-csi/intent.yaml` contains only repo-safe deployment intent.
- `/etc/nas-csi/selections.yaml` contains target-host selections and is not
  committed to the repo.

## Host Install

Run a dry-run first:

```sh
nas-csi-host-agent host-install \
  --intent /etc/nas-csi/intent.yaml \
  --selections /etc/nas-csi/selections.yaml
```

Review the generated config, reconcile decisions, skips, and refusals. Then
execute:

```sh
nas-csi-host-agent host-install \
  --intent /etc/nas-csi/intent.yaml \
  --selections /etc/nas-csi/selections.yaml \
  --execute
```

After rebooting TrueNAS, verify persistent host state:

```sh
nas-csi-host-agent host-install \
  --post-reboot-check \
  --config /etc/nas-csi/host.yaml
```

## Cluster Install

Run a dry-run:

```sh
nas-csi-host-agent cluster install \
  --config /etc/nas-csi/host.yaml
```

Then execute:

```sh
nas-csi-host-agent cluster install \
  --config /etc/nas-csi/host.yaml \
  --execute
```

Verify basic cluster status:

```sh
nas-csi-host-agent cluster status \
  --config /etc/nas-csi/host.yaml

kubectl --kubeconfig /etc/nas-csi/kubeconfig get nodes -o wide
```

## CSI Install

Run a dry-run:

```sh
nas-csi-host-agent csi install \
  --config /etc/nas-csi/host.yaml
```

Then execute:

```sh
nas-csi-host-agent csi install \
  --config /etc/nas-csi/host.yaml \
  --execute
```

This installs node runtime config, applies the `nas-csi` substrate manifest,
creates static PV/PVC objects for configured existing datasets, and runs smoke
checks. It must not create, delete, or rewrite authoritative datasets.

## Workload Validation

Run real workload validation after CSI smoke checks pass:

```sh
nas-csi-host-agent workload validate \
  --config /etc/nas-csi/host.yaml \
  --execute
```

Review the report under
`/var/lib/nas-csi/rendered/workload-validation/report.txt`.

## Observability Checks

Check host-side logs:

```sh
journalctl -u nas-csi-host-agent --since today
journalctl -u 'nascsi-virtiofsd-*' --since today
```

Check Kubernetes-side logs and resource usage:

```sh
kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  -n kube-system logs deployment/nas-csi-controller -c nas-csi-controller

kubectl --kubeconfig /etc/nas-csi/kubeconfig \
  -n kube-system logs daemonset/nas-csi-node -c nas-csi-node

kubectl --kubeconfig /etc/nas-csi/kubeconfig top nodes
kubectl --kubeconfig /etc/nas-csi/kubeconfig -n kube-system top pods
```

## Completion Criteria

- `host-install --post-reboot-check` passes.
- `cluster install --execute` is idempotent.
- `csi install --execute` verifies static existing-dataset mounts.
- `workload validate --execute` passes for the repository and content exports.
- TrueNAS snapshot, replication, quota, retention, SMB share definitions, and
  permissions still see the datasets as normal TrueNAS-owned datasets.
