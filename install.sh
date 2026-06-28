#!/usr/bin/env bash
set -euo pipefail

REPO="Cyberdog-AI-Lab/aidd-workflow"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"

# Detect OS and architecture
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "${ARCH}" in
  x86_64)        ARCH_TAG="x86_64" ;;
  arm64|aarch64) ARCH_TAG="aarch64" ;;
  *)
    echo "Unsupported architecture: ${ARCH}" >&2
    exit 1
    ;;
esac

case "${OS}" in
  linux)  TARGET="${ARCH_TAG}-unknown-linux-musl" ;;
  darwin) TARGET="${ARCH_TAG}-apple-darwin" ;;
  *)
    echo "Unsupported OS: ${OS}" >&2
    exit 1
    ;;
esac

# Resolve version: use VERSION env var or fetch latest release tag
if [ -z "${VERSION:-}" ]; then
  if command -v curl &>/dev/null; then
    VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/')"
  elif command -v wget &>/dev/null; then
    VERSION="$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | sed 's/.*"tag_name": *"\(.*\)".*/\1/')"
  else
    echo "curl or wget is required" >&2
    exit 1
  fi
fi

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

install_binary() {
  local binary="$1"
  local archive="${binary}-${TARGET}.tar.gz"
  local url="https://github.com/${REPO}/releases/download/${VERSION}/${archive}"

  echo "Installing ${binary} ${VERSION} for ${TARGET}..."

  if command -v curl &>/dev/null; then
    curl -fsSL "${url}" -o "${TMP}/${archive}"
  else
    wget -qO "${TMP}/${archive}" "${url}"
  fi

  tar -xzf "${TMP}/${archive}" -C "${TMP}"
  mkdir -p "${INSTALL_DIR}"
  mv "${TMP}/${binary}" "${INSTALL_DIR}/${binary}"
  chmod +x "${INSTALL_DIR}/${binary}"

  echo "Installed ${INSTALL_DIR}/${binary}"
}

install_binary "workflow-runner"
install_binary "webhook-mcp"

# Add INSTALL_DIR to PATH via shell rc file if not already present
if ! echo "${PATH}" | tr ':' '\n' | grep -qx "${INSTALL_DIR}"; then
  case "${SHELL:-}" in
    */zsh)  RC_FILE="${HOME}/.zshrc" ;;
    */bash) RC_FILE="${HOME}/.bashrc" ;;
    *)      RC_FILE="${HOME}/.profile" ;;
  esac

  PATH_LINE="export PATH=\"${INSTALL_DIR}:\${PATH}\""

  if [ -f "${RC_FILE}" ] && grep -qF "${PATH_LINE}" "${RC_FILE}"; then
    echo "${INSTALL_DIR} is already configured in ${RC_FILE}"
  else
    printf '\n# Added by aidd-workflow installer\n%s\n' "${PATH_LINE}" >> "${RC_FILE}"
    echo "Added ${INSTALL_DIR} to PATH in ${RC_FILE}"
    echo "Run 'source ${RC_FILE}' or restart your shell to use the installed binaries"
  fi
fi
