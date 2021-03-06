#!/usr/bin/env bash
#
# |cargo install| of the top-level crate will not install binaries for
# other workspace crates or native program crates.
set -e

if [[ -z $1 ]]; then
  echo Install directory not specified
  exit 1
fi
installDir="$(mkdir -p "$1"; cd "$1"; pwd)"
cargoFeatures="$2"
echo "Install location: $installDir"

cd "$(dirname "$0")"/..

SECONDS=0

(
  set -x
  cargo build --all --release --features="$cargoFeatures"
)

BIN_CRATES=(
  drone
  keygen
  fullnode
  bench-streamer
  bench-tps
  fullnode-config
  genesis
  ledger-tool
  wallet
)

for crate in "${BIN_CRATES[@]}"; do
  (
    set -x
    cargo install --force --path "$crate" --root "$installDir" --features="$cargoFeatures"
  )
done

for dir in programs/native/*; do
  for program in echo target/release/deps/lib{,solana_}"$(basename "$dir")"{,_program}.{so,dylib,dll}; do
    if [[ -f $program ]]; then
      mkdir -p "$installDir/bin/deps"
      rm -f "$installDir/bin/deps/$(basename "$program")"
      cp -v "$program" "$installDir"/bin/deps
    fi
  done
done

du -a "$installDir"
echo "Done after $SECONDS seconds"
