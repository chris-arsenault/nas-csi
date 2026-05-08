#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PREFIX=${PREFIX:-/usr/local}
CONFIG_DIR=${CONFIG_DIR:-/etc/nas-csi}
STATE_DIR=${STATE_DIR:-/var/lib/nas-csi}
LOG_DIR=${LOG_DIR:-/var/log/nas-csi}
RUNTIME_DIR=${RUNTIME_DIR:-/run/nas-csi}
SYSTEMD_DIR=${SYSTEMD_DIR:-/etc/systemd/system}
BINARY=${BINARY:-"$SCRIPT_DIR/bin/nas-csi-host-agent"}

install -d -m 0755 "$PREFIX/sbin"
install -d -m 0750 "$CONFIG_DIR" "$STATE_DIR" "$LOG_DIR" "$RUNTIME_DIR"
install -d -m 0700 "$CONFIG_DIR/secrets"
install -m 0755 "$BINARY" "$PREFIX/sbin/nas-csi-host-agent"
install -m 0644 "$SCRIPT_DIR/nas-csi-host-agent.service" "$SYSTEMD_DIR/nas-csi-host-agent.service"

if [ ! -f "$CONFIG_DIR/host-agent.env" ]; then
  install -m 0640 "$SCRIPT_DIR/nas-csi-host-agent.env" "$CONFIG_DIR/host-agent.env"
fi

systemctl daemon-reload
printf '%s\n' "Installed nas-csi-host-agent."
printf '%s\n' "Edit $CONFIG_DIR/host.yaml and $CONFIG_DIR/host-agent.env before enabling the service."

