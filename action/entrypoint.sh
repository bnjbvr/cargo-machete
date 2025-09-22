#!/bin/bash

set -eu

# Determine the Rust target triple using rustc.
echo "Determining Rust target triple..."
TRIPLE=$(rustc -vV | grep host | awk '{print $2}')
if [[ "$TRIPLE" == *"unknown-linux"* && "$TRIPLE" == *"x86_64"* ]]; then
  # Replace 'gnu' with 'musl' only for linux x86_64.
  TRIPLE="${TRIPLE/gnu/musl}"
fi
echo "Found TRIPLE=${TRIPLE}."

# Fetch the latest version of cargo-machete from GitHub releases.
VERSION=$(curl -s https://api.github.com/repos/bnjbvr/cargo-machete/releases/latest | jq -r .tag_name)
echo "Found VERSION=${VERSION}."

# Download the precompiled binary if available, otherwise fall back to cargo install.
ARCHIVE_URL="https://github.com/bnjbvr/cargo-machete/releases/download/${VERSION}/cargo-machete-${VERSION}-${TRIPLE}.tar.gz"

if curl --output /dev/null --silent --head --fail "$ARCHIVE_URL"; then
  echo "Downloading precompiled binary from ${ARCHIVE_URL} …"
  curl -L -o cargo-machete.tar.gz "$ARCHIVE_URL"
  tar -xzf cargo-machete.tar.gz
  mv cargo*/cargo-machete /usr/local/bin/cargo-machete
  chmod +x /usr/local/bin/cargo-machete
  echo "cargo-machete downloaded and extracted successfully."
else
  echo "Precompiled binary not found for $TRIPLE. Falling back to cargo install."
  cargo install cargo-machete
fi

# Finally, test the installation.
if command -v cargo-machete; then
  echo "cargo-machete installed successfully."
else
  echo "cargo-machete installation failed."
  exit 1
fi
