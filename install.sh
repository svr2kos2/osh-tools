#!/usr/bin/env bash
set -euo pipefail

REPO="svr2kos2/osh-tools"
ASSET_NAME="osh-tools-linux-x86_64.tar.gz"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/osh"
CONFIG_FILE="${CONFIG_DIR}/devices.json"
SERVICE_FILE="/etc/systemd/system/osh-relay.service"
PROFILE_FILE="/etc/profile.d/osh.sh"

RUN_USER="${SUDO_USER:-${USER}}"

if [ "${EUID}" -ne 0 ]; then
  echo "Please run as root (sudo ./install.sh)."
  exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required." >&2
  exit 1
fi

if ! command -v tar >/dev/null 2>&1; then
  echo "tar is required." >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

API_URL="https://api.github.com/repos/${REPO}/releases/latest"
ASSET_URL="$(curl -fsSL "${API_URL}" | grep -m 1 "\"browser_download_url\":.*${ASSET_NAME}\"" | cut -d '"' -f 4)"

if [ -z "${ASSET_URL}" ]; then
  echo "Failed to locate release asset ${ASSET_NAME}." >&2
  exit 1
fi

echo "Downloading ${ASSET_NAME}..."
curl -fL "${ASSET_URL}" -o "${TMP_DIR}/${ASSET_NAME}"

echo "Installing binaries to ${INSTALL_DIR}..."
mkdir -p "${INSTALL_DIR}"
tar -xzf "${TMP_DIR}/${ASSET_NAME}" -C "${TMP_DIR}"
install -m 0755 "${TMP_DIR}/osh-relay" "${INSTALL_DIR}/osh-relay"
install -m 0755 "${TMP_DIR}/osh" "${INSTALL_DIR}/osh"
install -m 0755 "${TMP_DIR}/osh-admin" "${INSTALL_DIR}/osh-admin"

if [ ! -f "${PROFILE_FILE}" ]; then
  echo "export PATH=\"/usr/local/bin:\$PATH\"" > "${PROFILE_FILE}"
  chmod 0644 "${PROFILE_FILE}"
fi

if [ ! -f "${CONFIG_FILE}" ]; then
  mkdir -p "${CONFIG_DIR}"
  SECRET_KEY="$(openssl rand -hex 16 2>/dev/null || cat /proc/sys/kernel/random/uuid | tr -d '-')"
  cat > "${CONFIG_FILE}" <<EOF
{
  "max_devices": 16,
  "secret_key": "${SECRET_KEY}",
  "devices": []
}
EOF
  chmod 0640 "${CONFIG_FILE}"
  chown "${RUN_USER}":"${RUN_USER}" "${CONFIG_FILE}" || true
fi

cat > "${SERVICE_FILE}" <<EOF
[Unit]
Description=OSH Relay Service
After=network.target

[Service]
Type=simple
User=${RUN_USER}
ExecStart=${INSTALL_DIR}/osh-relay --config ${CONFIG_FILE}
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
  systemctl enable --now osh-relay
  systemctl status --no-pager osh-relay || true
else
  echo "systemctl not found. Please start osh-relay manually." >&2
fi

echo "Install complete."