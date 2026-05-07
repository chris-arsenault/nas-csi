# Contributing

`nas-csi` is still in an early implementation phase. Keep changes small,
explicit, and aligned with the one-host TrueNAS target topology.

## Development Workflow

1. Make focused changes.
2. Keep generated or host-local state out of git.
3. Run the full check before committing:

   ```bash
   cargo run -p nas-csi-xtask -- check
   ```

4. Update docs when behavior or ownership boundaries change.

## Repository Boundaries

Allowed in the repo:

- schematic intent examples;
- fictional discovery and host config fixtures;
- docs, tests, and source code;
- deployment scaffolding that does not contain real host facts.

Not allowed in the repo:

- real TrueNAS pool or dataset names;
- LAN IPs, DNS names, or MAC addresses from a real deployment;
- API keys, kubeconfigs, k3s tokens, or SSH keys;
- generated `.nas-csi` state.

## Documentation Layout

- `docs/design/`: architecture and design decisions.
- `docs/project/`: implementation plan and repository structure.
- `docs/operations/`: runbooks and operator procedures.
- `docs/research/`: source-backed research notes and constraints.
- `examples/`: fictional fixtures for tests and documentation.
