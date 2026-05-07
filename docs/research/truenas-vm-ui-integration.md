# TrueNAS VM UI Integration

## Question

Can `nas-csi-host-agent` create libvirt domains directly and then post-register
them into the TrueNAS VM UI for visibility, resource utilization, and console
access?

## Current Answer

Probably not through a supported public API.

The TrueNAS v27 API documents VM CRUD and status methods around middleware VM
records:

- `vm.create`
- `vm.query`
- `vm.get_instance`
- `vm.update`
- `vm.delete`
- `vm.start`
- `vm.stop`
- `vm.status`
- `vm.get_console`
- `vm.get_memory_usage`

The VM create/query schemas include VM device records, but the documented device
types are CD-ROM, display, NIC, PCI, raw, disk, and USB. There is no documented
first-class virtiofs filesystem device. `vm.create` does expose
`command_line_args`, but that is not the same as a clean ownership model because
virtiofs also depends on shared-memory backing and stable socket/device
reconciliation.

The v27 API index does not show a `vm.import`, `vm.adopt`, or similar method for
registering an existing external libvirt domain.

## Implication

For v1, host-agent-owned VMs should stay outside the TrueNAS VM UI. The agent
should expose its own operational surface for these nodes.

## What We Can Still Use From TrueNAS

TrueNAS has useful APIs for middleware-owned VMs:

- `vm.status` returns runtime state and PID for a VM ID.
- `vm.get_console` returns console connection information.
- `vm.get_memory_usage` returns current VM memory usage.
- `reporting.realtime` streams host CPU, network, memory, disk, and ZFS
  statistics.

These are relevant if a later compatibility mode creates VMs through TrueNAS
middleware. They do not solve external-domain registration by themselves.

## Possible Experiments

### Experiment A: Middleware-Owned VM With Command-Line Args

Create a VM through `vm.create`, set `command_line_args`, and see whether the
required virtiofs QEMU configuration can be represented and survives:

- start;
- stop;
- restart;
- UI edit;
- middleware restart;
- host reboot;
- TrueNAS upgrade simulation where possible.

Reject this if the UI or middleware can drop required storage transport state.

### Experiment B: External Domain Discovery

Create a libvirt domain directly and check whether any TrueNAS API or UI surface
discovers it. If it appears only in `virsh` and not `vm.query`, assume TrueNAS UI
integration is not supported for host-agent-owned domains.

### Experiment C: Host-Agent Operational UI

Expose the useful parts ourselves:

- HTTP page or CLI for VM status and actions;
- Prometheus metrics;
- serial console helper;
- SSH helper;
- qemu guest agent status;
- virtiofs export health.

This is the preferred path if TrueNAS registration is unsupported.

Sources:

- <https://api.truenas.com/v27.0/api_methods_vm.create.html>
- <https://api.truenas.com/v27.0/api_methods_vm.query.html>
- <https://api.truenas.com/v27.0/api_methods_vm.status.html>
- <https://api.truenas.com/v27.0/api_methods_vm.get_console.html>
- <https://api.truenas.com/v27.0/api_methods_vm.get_memory_usage.html>
- <https://api.truenas.com/v27.0/api_events_reporting.realtime.html>
