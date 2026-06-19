#!/usr/bin/env bash
set -Eeuo pipefail

RUST_TOOLCHAIN="${ORDO_RUST_TOOLCHAIN:-1.93.0}"
NODE_MAJOR="${ORDO_NODE_MAJOR:-24}"

as_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
    return
  fi

  if ! command -v sudo >/dev/null 2>&1; then
    echo "Missing sudo. Install prerequisites as root, then rerun Ordo install." >&2
    exit 1
  fi

  sudo "$@"
}

have_apt() {
  command -v apt-get >/dev/null 2>&1
}

install_debian_build_deps() {
  if ! have_apt; then
    echo "No apt-get found; skipping Debian/Ubuntu package install."
    return
  fi

  echo "Installing Ordo Linux build prerequisites..."
  as_root env DEBIAN_FRONTEND=noninteractive apt-get update
  as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y \
    ca-certificates \
    curl \
    git \
    build-essential \
    pkg-config \
    libssl-dev \
    libx11-dev \
    libxcb1-dev \
    libxkbcommon-dev \
    libwayland-dev \
    libegl1-mesa-dev \
    libgles2-mesa-dev
}

node_major_version() {
  node -p 'Number(process.versions.node.split(".")[0])' 2>/dev/null || printf '0\n'
}

ensure_node() {
  local current_major
  current_major="$(node_major_version)"
  if command -v node >/dev/null 2>&1 \
    && command -v npm >/dev/null 2>&1 \
    && [[ "$current_major" =~ ^[0-9]+$ ]] \
    && (( current_major >= 20 )); then
    echo "Node.js is available: $(node --version)"
    echo "npm is available: $(npm --version)"
    return
  fi

  if ! have_apt; then
    echo "Node.js 20+ and npm are required, but automatic install is only wired for apt-based distros." >&2
    exit 1
  fi

  echo "Installing Node.js ${NODE_MAJOR}.x from NodeSource..."
  local setup_script
  setup_script="$(mktemp)"
  curl -fsSL "https://deb.nodesource.com/setup_${NODE_MAJOR}.x" -o "$setup_script"
  as_root bash "$setup_script"
  rm -f "$setup_script"
  as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y nodejs

  current_major="$(node_major_version)"
  if ! command -v node >/dev/null 2>&1 \
    || ! command -v npm >/dev/null 2>&1 \
    || [[ ! "$current_major" =~ ^[0-9]+$ ]] \
    || (( current_major < 20 )); then
    echo "Node.js 20+ install failed or produced an unsupported version." >&2
    exit 1
  fi
}

ensure_rust() {
  if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
    echo "Cargo is available: $(cargo --version)"
    echo "rustc is available: $(rustc --version)"
    return
  fi

  if command -v rustup >/dev/null 2>&1; then
    echo "Installing Rust toolchain ${RUST_TOOLCHAIN} with rustup..."
    rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
    rustup default "$RUST_TOOLCHAIN"
  else
    echo "Installing Rust/Cargo with rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
  fi

  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
  fi

  if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    echo "Rust/Cargo install completed, but cargo is still not on PATH." >&2
    echo "Close and reopen the terminal, or run: source \"\$HOME/.cargo/env\"" >&2
    exit 1
  fi
}

validate_commands() {
  local missing=0
  for command_name in git curl cargo rustc node npm pkg-config; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
      echo "Missing required command after dependency install: $command_name" >&2
      missing=1
    fi
  done

  if have_apt && ! command -v dpkg-deb >/dev/null 2>&1; then
    echo "Missing required command after dependency install: dpkg-deb" >&2
    missing=1
  fi

  if (( missing )); then
    exit 1
  fi
}

install_debian_build_deps
ensure_node
ensure_rust
validate_commands

echo "Ordo Linux build prerequisites are ready."
