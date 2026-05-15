#!/usr/bin/env bash
set -euo pipefail

REPO="cyberdog/aidd-workflow"
BINARY="workflow-runner"
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

ARCHIVE="${BINARY}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

echo "Installing ${BINARY} ${VERSION} for ${TARGET}..."
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

if command -v curl &>/dev/null; then
  curl -fsSL "${URL}" -o "${TMP}/${ARCHIVE}"
else
  wget -qO "${TMP}/${ARCHIVE}" "${URL}"
fi

tar -xzf "${TMP}/${ARCHIVE}" -C "${TMP}"
mkdir -p "${INSTALL_DIR}"
mv "${TMP}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
chmod +x "${INSTALL_DIR}/${BINARY}"

echo "Installed ${INSTALL_DIR}/${BINARY}"
if ! echo "${PATH}" | grep -q "${INSTALL_DIR}"; then
  echo "Add ${INSTALL_DIR} to your PATH to use ${BINARY}"
fi
