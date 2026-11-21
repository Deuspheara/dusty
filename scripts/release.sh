#!/bin/sh
# Bump the version everywhere, commit, tag and push. CI builds and publishes the release.
#   scripts/release.sh 0.2.0           stable release
#   scripts/release.sh 0.2.0-beta.1    pre-release: published as a GitHub pre-release on the beta update channel
set -e
cd "$(dirname "$0")/.."
V="$1"
echo "$V" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|rc)\.[0-9]+)?$' || { echo "usage: $0 X.Y.Z[-beta.N]" >&2; exit 2; }
git diff --quiet && git diff --cached --quiet || { echo "commit or stash your changes first" >&2; exit 1; }

sed -i '' "s/^version = \".*\"/version = \"$V\"/" app/Cargo.toml
(cd app && cargo update --workspace --offline >/dev/null 2>&1 || cargo generate-lockfile >/dev/null)

git add app/Cargo.toml app/Cargo.lock
git commit -m "chore: release v$V"
git tag -a "v$V" -m "Dusty $V"
git push origin HEAD "v$V"
echo "Tagged v$V. The Release workflow will publish Dusty-$V.dmg and refresh the update feed."
