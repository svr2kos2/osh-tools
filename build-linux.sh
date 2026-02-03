#!/bin/bash
# Build script for OSH components on Linux (OpenCloudOS)
# Run this on the VPS to build osh-relay and osh-cli

set -e

echo "Building OSH components for Linux x64..."

# Build osh-relay and osh-cli
cargo build --release -p osh-relay -p osh-cli

echo ""
echo "Build completed successfully!"
echo ""
echo "Binaries are located at:"
echo "  - ./target/release/osh-relay"
echo "  - ./target/release/osh"
echo "  - ./target/release/osh-admin"
echo ""
echo "To install, copy binaries to /usr/local/bin/:"
echo "  sudo cp target/release/osh-relay /usr/local/bin/"
echo "  sudo cp target/release/osh /usr/local/bin/"
echo "  sudo cp target/release/osh-admin /usr/local/bin/"
