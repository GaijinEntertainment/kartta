#!/bin/sh
set -eu

# Builds the addon on a musl host and runs the Node binding's tests against it. Runs inside the image of the `node-musl`
# job of .github/workflows/ci.yml, with a Node toolchain mounted at /opt/node.
export PATH="/opt/node/bin:$PATH"
cd bindings/node

npm ci --ignore-scripts
npm run build
npm test
