#!/bin/sh
# Bump the version everywhere, commit, tag and push. CI builds and publishes the release.
#   scripts/release.sh 0.2.0
set -e
cd "$(dirname "$0")/.."
V="$1"
echo "$V" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "usage: $0 X.Y.Z" >&2; exit 2; }
git diff --quiet && git diff --cached --quiet || { echo "commit or stash your changes first" >&2; exit 1; }

sed -i '' "s/^version = \".*\"/version = \"$V\"/" app/Cargo.toml
(cd app && cargo update -p dusty --offline >/dev/null 2>&1 || cargo generate-lockfile >/dev/null)
npm version "$V" --no-git-tag-version >/dev/null
sed -i '' "s/\.version(\"[^\"]*\")/.version(\"$V\")/" src/index.ts

git add app/Cargo.toml app/Cargo.lock package.json package-lock.json src/index.ts
git commit -m "chore: release v$V"
git tag -a "v$V" -m "Dusty $V"
git push origin HEAD "v$V"
echo "Tagged v$V. The Release workflow will publish Dusty-$V.dmg."
