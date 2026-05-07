#!/usr/bin/env python3
"""Validate example host-agent YAML files."""

from pathlib import Path
import sys

import yaml


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    intent_dir = root / "examples" / "intents"
    host_config_dir = root / "examples" / "configs"
    ok = True

    for path in sorted(intent_dir.glob("*.yaml")):
        with path.open("r", encoding="utf-8") as f:
            data = yaml.safe_load(f)

        errors = []
        if data.get("apiVersion") != "nas-csi.dev/v1alpha1":
            errors.append("apiVersion must be nas-csi.dev/v1alpha1")
        if data.get("kind") != "ClusterIntent":
            errors.append("kind must be ClusterIntent")
        if data.get("profile") not in {
            "maintenance-basic",
            "maintenance-control-plane",
        }:
            errors.append("profile must be a supported maintenance profile")
        nodes = data.get("nodes", {})
        if not isinstance(nodes.get("servers"), int) or nodes["servers"] < 1:
            errors.append("nodes.servers must be a positive integer")
        if not isinstance(nodes.get("agents"), int) or nodes["agents"] < 0:
            errors.append("nodes.agents must be a non-negative integer")
        if data.get("applications") != []:
            errors.append("applications must be an empty list")

        if errors:
            ok = False
            for error in errors:
                print(f"{path}: {error}", file=sys.stderr)
        else:
            print(
                f"ok {path.relative_to(root)}: "
                f"profile={data['profile']} "
                f"servers={data['nodes']['servers']} "
                f"agents={data['nodes']['agents']}"
            )

    for path in sorted(host_config_dir.glob("*.yaml")):
        with path.open("r", encoding="utf-8") as f:
            data = yaml.safe_load(f)

        errors = []
        if data.get("apiVersion") != "nas-csi.dev/v1alpha1":
            errors.append("apiVersion must be nas-csi.dev/v1alpha1")
        kind = data.get("kind")
        if kind == "HostConfig":
            if data.get("cluster", {}).get("distribution") != "k3s":
                errors.append("cluster.distribution must be k3s")
            if not data.get("nodes"):
                errors.append("nodes must not be empty")
            if not data.get("exports"):
                errors.append("exports must not be empty")

            node_names = set()
            macs = set()
            export_ids = set(data.get("exports", {}).keys())
            for node in data.get("nodes", []):
                name = node.get("name")
                mac = node.get("network", {}).get("mac")
                if name in node_names:
                    errors.append(f"duplicate node name {name}")
                node_names.add(name)
                if mac in macs:
                    errors.append(f"duplicate MAC {mac}")
                macs.add(mac)
                for export_id in node.get("exports", []):
                    if export_id not in export_ids:
                        errors.append(f"node {name} references missing export {export_id}")
        elif kind == "HostSelections":
            if not data.get("libvirt", {}).get("bridge"):
                errors.append("libvirt.bridge must not be empty")
            if not data.get("cluster", {}).get("version"):
                errors.append("cluster.version must not be empty")
            if not data.get("exports"):
                errors.append("exports must not be empty")
        elif kind == "DiscoveryInventory":
            if "host" not in data:
                errors.append("host must be present")
            if "truenas" not in data:
                errors.append("truenas must be present")
        else:
            errors.append("kind must be HostConfig, HostSelections, or DiscoveryInventory")

        if errors:
            ok = False
            for error in errors:
                print(f"{path}: {error}", file=sys.stderr)
        else:
            if kind == "HostConfig":
                detail = (
                    f"cluster={data['cluster']['name']} "
                    f"nodes={len(data['nodes'])} "
                    f"exports={len(data['exports'])}"
                )
            elif kind == "HostSelections":
                detail = (
                    f"cluster={data['cluster']['name']} "
                    f"exports={len(data['exports'])}"
                )
            else:
                detail = (
                    f"datasets={len(data['truenas'].get('filesystemDatasets', []))} "
                    f"bridges={len(data['network'].get('bridges', []))}"
                )
            print(f"ok {path.relative_to(root)}: {detail}")

    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
