#!/bin/sh
# Run only in the disposable image build stage, never on Home Assistant.
set -eu
inventory=/usr/share/doc/vistoda/runtime/debian-packages.tsv
destination=/corresponding-source
test -s "$inventory"
mkdir -p "$destination/packages"
cp "$inventory" "$destination/runtime-packages.tsv"
awk -F '\t' '{name=$3; version=$4; if(name=="")name=$1; if(version=="")version=$2; sub(/:[^:]+$/, "", name); print name "=" version}' \
    "$inventory" | sort -u > "$destination/source-versions.txt"
sed -i 's/^Types: deb$/Types: deb deb-src/' /etc/apt/sources.list.d/debian.sources
apt-get -o Acquire::Retries=2 update
cd "$destination/packages"
while IFS= read -r package; do
    case "$package" in *[!a-zA-Z0-9.+:~=_-]*|'') echo 'Invalid source identity' >&2; exit 1;; esac
    # POSIX file-size limits use 512-byte blocks; leave room for tar headers.
    ulimit -c 0
    ulimit -f 2200000
    apt-get -o Acquire::Retries=2 --download-only source "$package"
    test "$(du -sk "$destination" | cut -f1)" -le 1048576 || {
        echo 'Corresponding source exceeds the reviewed 1 GiB budget' >&2
        exit 1
    }
done < "$destination/source-versions.txt"
find . -type f -exec sha256sum {} + > "$destination/SHA256SUMS"
cd /
ulimit -f 2200000
tar -cf /corresponding-source.tar corresponding-source
sha256sum corresponding-source.tar > /corresponding-source.tar.sha256
