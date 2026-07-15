#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RESOURCE_DIR="${ROOT_DIR}/src-tauri/resources"
RUNTIME_DIR="${ROOT_DIR}/runtime-template"
NODE_VERSION="${NODE_VERSION:-24.13.0}"
NODE_ARCHIVE="node-v${NODE_VERSION}-darwin-arm64.tar.gz"
NODE_URL="https://nodejs.org/dist/v${NODE_VERSION}/${NODE_ARCHIVE}"

mkdir -p "${RESOURCE_DIR}"

if [[ ! -x "${RESOURCE_DIR}/node" ]]; then
  TEMP_DIR="$(mktemp -d)"
  trap 'rm -rf "${TEMP_DIR}"' EXIT
  curl --fail --location "${NODE_URL}" --output "${TEMP_DIR}/${NODE_ARCHIVE}"
  tar -xzf "${TEMP_DIR}/${NODE_ARCHIVE}" -C "${TEMP_DIR}"
  cp "${TEMP_DIR}/node-v${NODE_VERSION}-darwin-arm64/bin/node" "${RESOURCE_DIR}/node"
  chmod +x "${RESOURCE_DIR}/node"
fi

(
  cd "${RUNTIME_DIR}"
  yarn install --non-interactive

  COMMAND_FILE="node_modules/@nocobase/plugin-file-manager/dist/server/commands/repair-filenames.js"
  if [[ -f "${COMMAND_FILE}" ]] && ! grep -q 'module.exports.default = registerRepairFilenamesCommand' "${COMMAND_FILE}"; then
    printf '\nmodule.exports.default = registerRepairFilenamesCommand;\n' >> "${COMMAND_FILE}"
  fi
)

tar \
  --exclude='.DS_Store' \
  --exclude='._*' \
  -czf "${RESOURCE_DIR}/runtime.tar.gz" \
  -C "${RUNTIME_DIR}" \
  .

echo "Prepared ${RESOURCE_DIR}/node and ${RESOURCE_DIR}/runtime.tar.gz"
