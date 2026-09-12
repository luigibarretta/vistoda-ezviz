#!/bin/sh
# Record the actual runtime packages; do not infer FFmpeg's license from Cargo.
set -eu
destination=$1
mkdir -p "$destination"
dpkg-query -W -f='${binary:Package}\t${Version}\t${source:Package}\t${source:Version}\n' \
    > "$destination/debian-packages.tsv"
ffmpeg -version > "$destination/ffmpeg-version.txt" 2>&1
ffmpeg -buildconf > "$destination/ffmpeg-buildconf.txt" 2>&1
ffmpeg -L > "$destination/ffmpeg-license.txt" 2>&1
test -s /usr/share/doc/ffmpeg/copyright
cp /usr/share/doc/ffmpeg/copyright "$destination/ffmpeg-copyright"
(cd "$destination" && sha256sum ./*.tsv ./*.txt ffmpeg-copyright > SHA256SUMS)
