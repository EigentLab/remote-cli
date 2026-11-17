#!/bin/sh
# Builds the Server and Dashboard images, tagged with the workspace version and `latest`. Run it on the build
# server from a source tree; it pushes nothing.
#
#   sudo packaging/docker/build-images.sh
#   -> ghcr.io/eigentlab/remote-cli-server:<version>, :latest and ghcr.io/eigentlab/remote-cli-dashboard:<version>, :latest
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)
REPO=ghcr.io/eigentlab/remote-cli

docker build -t "$REPO-server:$VERSION" -t "$REPO-server:latest" "$ROOT"
docker build -t "$REPO-dashboard:$VERSION" -t "$REPO-dashboard:latest" "$ROOT/dashboard"
docker image ls "$REPO-*"
