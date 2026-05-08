#!/bin/sh
set -eu

PREFIX=${PREFIX:-/usr/local}
SYSTEMD_DIR=${SYSTEMD_DIR:-/etc/systemd/system}

if systemctl list-unit-files nas-csi-host-agent.service >/dev/null 2>&1; then
  systemctl disable --now nas-csi-host-agent.service >/dev/null 2>&1 || true
fi

rm -f "$SYSTEMD_DIR/nas-csi-host-agent.service"
rm -f "$PREFIX/sbin/nas-csi-host-agent"
systemctl daemon-reload

printf '%s\n' "Removed nas-csi-host-agent unit and binary."
printf '%s\n' "Left /etc/nas-csi, /var/lib/nas-csi, /var/log/nas-csi, and all TrueNAS datasets untouched."

