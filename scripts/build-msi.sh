#!/usr/bin/env bash
# Build the Drivecap MSI installer.
#
# Prerequisites (one-time):
#   dotnet tool install --global wix
#   wix eula accept wix7
#   wix extension add --global WixToolset.Util.wixext WixToolset.UI.wixext
#
# Usage: ./scripts/build-msi.sh

set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

VERSION=$(grep '^version = ' "$REPO_ROOT/crates/drivecap-cli/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
OUT="$REPO_ROOT/target/wix/drivecap-${VERSION}-x64.msi"

echo "Building drivecap ${VERSION} release binaries..."
cargo build --release -p drivecap-cli --manifest-path "$REPO_ROOT/Cargo.toml"

echo "Building MSI at ${OUT}..."
mkdir -p "$REPO_ROOT/target/wix"
wix build -arch x64 \
  -d "Version=${VERSION}" \
  -d "CargoTargetBinDir=$REPO_ROOT/target/release" \
  -b "$REPO_ROOT/crates/drivecap-cli/wix" \
  "$REPO_ROOT/crates/drivecap-cli/wix/main.wxs" \
  -o "${OUT}" \
  -ext WixToolset.Util.wixext \
  -ext WixToolset.UI.wixext

echo "Done. MSI: ${OUT}"
