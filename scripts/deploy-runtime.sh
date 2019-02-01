#!/bin/bash

#
# Deploy built runtime wasm to a live networ
#
# Usage:
#   ./scripts/deploy-runtime.sh
set -ex

# Ensure clean clone
echo "Cloning cennz-cli..."
sudo rm -rf cennz-cli
git clone ssh://git@bitbucket.org/centralitydev/cennz-cli

# Deploy cennznet-runtime
docker run --rm \
  -v "$(pwd):/cennznet-node" \
  -w "/cennznet-node/cennz-cli" \
  node:alpine \
  sh -c \
  "yarn && \
  /cennznet-node/cennz-cli/bin/run repl \
  --endpoint=ws://cennznet-node-0.centrality.me:9944 \
    /cennznet-node/cennz-cli/scripts/upgrade-runtime.js \
    Centrality \
    /cennznet-node/runtime/wasm/target/wasm32-unknown-unknown/release/cennznet_runtime.compact.wasm"