#!/usr/bin/env bash
set -Eeuo pipefail

CHECK=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      CHECK=1
      shift
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

REPO_URL="${ORDO_REPO_URL:-https://github.com/Lucerna-Labs/ordo.git}"
BRANCH="${ORDO_BRANCH:-main}"
ORDO_DIR="${ORDO_DIR:-$HOME/ordo}"

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

need_command git

if [[ -d "$ORDO_DIR/.git" ]]; then
  echo "Updating existing Ordo clone:"
  echo "  $ORDO_DIR"
  cd "$ORDO_DIR"

  current_branch="$(git rev-parse --abbrev-ref HEAD)"
  if [[ "$current_branch" != "$BRANCH" ]]; then
    echo "Existing Ordo clone is on branch '$current_branch', not '$BRANCH'." >&2
    echo "Switch branches yourself, or set ORDO_BRANCH to the branch you want." >&2
    exit 1
  fi

  git fetch origin "$BRANCH"
  git pull --ff-only origin "$BRANCH"
elif [[ -e "$ORDO_DIR" ]]; then
  echo "Cannot install into $ORDO_DIR because it already exists but is not a Git clone." >&2
  echo "Move that folder, or choose another install folder with ORDO_DIR=/path/to/ordo." >&2
  exit 1
else
  echo "Cloning Ordo:"
  echo "  $REPO_URL"
  echo "Into:"
  echo "  $ORDO_DIR"
  git clone --branch "$BRANCH" "$REPO_URL" "$ORDO_DIR"
  cd "$ORDO_DIR"
fi

if [[ ! -f "./Install-Ordo-Linux.sh" ]]; then
  echo "Install-Ordo-Linux.sh is missing after update." >&2
  echo "Current commit: $(git rev-parse --short HEAD 2>/dev/null || true)" >&2
  exit 1
fi

chmod +x ./Install-Ordo-Linux.sh ./Build-Ordo-Linux-Deb.sh \
  ./Build-Ordo-Linux-Portable.sh ./Build-Ordo-Linux-AppImage.sh \
  ./scripts/install-linux-build-deps.sh 2>/dev/null || true

if [[ "${ORDO_SKIP_LINUX_DEPS:-0}" != "1" ]]; then
  if [[ ! -f "./scripts/install-linux-build-deps.sh" ]]; then
    echo "scripts/install-linux-build-deps.sh is missing after update." >&2
    exit 1
  fi
  ./scripts/install-linux-build-deps.sh
fi

if (( CHECK )); then
  ./Install-Ordo-Linux.sh --check
else
  ./Install-Ordo-Linux.sh
fi
