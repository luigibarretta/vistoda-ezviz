#!/bin/sh
# Preserve upstream license/notice files for every fetched Cargo dependency.
set -eu
destination=$1
mkdir -p "$destination"
cargo metadata --locked --format-version 1 >"$destination/cargo-metadata.json"
find /usr/local/cargo/registry/src -type f \( -iname '*license*' -o -iname '*copying*' -o -iname '*notice*' \) -exec sh -c '
    destination=$1
    shift
    for source_path do
        relative_path=${source_path#/usr/local/cargo/registry/src/}
        target_path="$destination/$relative_path"
        mkdir -p "${target_path%/*}"
        cp "$source_path" "$target_path"
    done
' sh "$destination" {} +
