#!/usr/bin/env bash
# Builds the archive to send to a friend: lockin.tar.gz, with no build files or personal data.
# It includes vendor/moodle when that exists on this machine.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
parts=(Cargo.toml Cargo.lock README.md install.sh src crates)
[ -d vendor ] && parts+=(vendor)
tar --create --gzip --file lockin.tar.gz --transform 's,^,lockin/,' "${parts[@]}"
echo "Created $(pwd)/lockin.tar.gz ($(du -h lockin.tar.gz | cut -f1))"
echo "Your friend runs:  tar xf lockin.tar.gz && cd lockin && ./install.sh"
