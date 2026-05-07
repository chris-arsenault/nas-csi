# TrueNAS Virtiofs Integration

## What The Research Says

TrueNAS SCALE is a storage appliance first. Its documented VM storage path is
not "pass a mounted host dataset into a VM." The VM docs say VMs do not directly
communicate with host NAS storage by default and point users toward network
access through a bridge. That is the exact gap this project fills.

The current API surface is useful but incomplete for this design:

- JSON-RPC over WebSocket is the right management API.
- Dataset, snapshot, SMB, service, and VM query APIs are available.
- VM device query docs list disks, raw devices, NICs, display, PCI, and similar
  device types, but not a first-class virtiofs filesystem device.
- VM creation docs include `command_line_args`; this might help on some
  versions, but it is not enough to assume correctness because virtiofs also
  requires VM shared-memory backing and careful device wiring.

Community reports matter here because they describe the operational failure
mode: hand-edited libvirt XML can disappear after the VM is restarted through
the TrueNAS UI. That is not surprising for an appliance whose middleware owns
its configuration database.

## Design Consequence

Do not rely on manual XML edits as persistent state.

The host agent must own virtiofs reconciliation. The preferred design now goes
further: the host agent owns the Kubernetes node VM domains themselves. It can
still support adoption of a TrueNAS UI VM later, but that is compatibility mode,
not the primary path.

## Supported Integration Modes

### Mode A: Host-Agent Managed VM

The host agent owns the Kubernetes node VM's libvirt domain. TrueNAS manages
datasets and SMB; the host agent manages this VM.

Advantages:

- Deterministic domain XML.
- Full control over memory backing, PCI topology, filesystem devices, and
  externally launched `virtiofsd` sockets.
- Disposable node rebuilds.
- Less fighting with middleware.

Risks:

- VM lifecycle moves out of the TrueNAS UI.
- The agent must own install, start, stop, and recovery behavior.

For this project, this is the preferred path.

### Mode B: Adopted TrueNAS VM

Use a VM created in the TrueNAS UI. The host agent discovers it with TrueNAS API
and libvirt, then reconciles virtiofs state.

Advantages:

- Keeps the VM visible in TrueNAS UI.
- Lower initial disruption.

Risks:

- TrueNAS may regenerate VM definitions.
- Required shared-memory backing may need VM restart.
- `command_line_args` may not be sufficient or may interact poorly with
  middleware-generated QEMU arguments.

## Lab Questions

These must be answered on the actual TrueNAS host:

- What TrueNAS version is installed?
- What QEMU and libvirt versions are present?
- Is Rust `virtiofsd` installed, and where?
- Does the VM already use shared-memory backing?
- Do project-owned libvirt domains survive TrueNAS middleware restart and host
  reboot without appliance cleanup?
- Can `command_line_args` persist the required virtiofs arguments without
  breaking TrueNAS VM lifecycle if compatibility mode is ever needed?
- Does live virtiofs hotplug work, or do exports need static-at-boot attachment?
- Does TrueNAS middleware overwrite agent changes after VM start, host boot, UI
  edit, or middleware restart?

## Initial Recommendation

Build the host agent around host-agent-owned VMs. Make the lab optimize for
static-at-boot attachment. Dynamic hotplug and adopted TrueNAS UI VMs are later
compatibility features, not v1 requirements.

Sources:

- <https://www.truenas.com/docs/scale/virtualmachines/managingvms/>
- <https://api.truenas.com/v27.0/jsonrpc.html>
- <https://api.truenas.com/v26.04.0/api_methods_vm.create.html>
- <https://api.truenas.com/v25.04/api_events_vm.device.query.html>
- <https://www.truenas.com/community/threads/scale-feature-request-file-system-passthrough.106673/>
- <https://www.truenas.com/community/threads/truenas-scale-removing-custom-libvirt-options.90553/>
