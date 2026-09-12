# Third-party material in distributions

The encrypted-RTP implementation includes MIT-licensed cloud-cam-viewer
adaptations. Preserve `third_party/cloud-cam-viewer` with source distributions.
Both image variants include it under `/usr/share/doc/vistoda/third_party`.
Do not describe this component as a certified independent/clean-room rewrite.

## Runtime packages and FFmpeg

Both image variants use the Debian FFmpeg executable as a separate remux process.
Its exact configuration and the distribution's package set are recorded during
the image build under `/usr/share/doc/vistoda/runtime`:

- `debian-packages.tsv`: binary package/version and source package/version;
- `ffmpeg-version.txt`, `ffmpeg-buildconf.txt`, `ffmpeg-license.txt`;
- `ffmpeg-copyright` and an integrity manifest `SHA256SUMS`.

Debian package copyright notices remain under `/usr/share/doc/<package>`.
Cargo notices are under `/usr/share/doc/vistoda/dependencies`.
The complete corresponding Debian source packages for the runtime inventory are
bundled in `/usr/share/doc/vistoda/corresponding-source.tar`, with an adjacent
SHA-256 file. This includes FFmpeg, the Debian patches/build material, and the
sources of the other installed Debian runtime packages. Extract the tar and use
`dpkg-source -x <package>.dsc` for a selected source package. You may use and
modify each source under its included license. No source download occurs on HA.

The disposable build stage resolves exact source-package versions from Debian's
authenticated indexes and fails if unavailable or if the 1 GiB budget is exceeded.
Preserve the source bundle with redistributed images/executables; inventory alone
is not a substitute. Per-package hashes are relative to the extracted `packages`
directory. Do not remove Debian copyright files when slimming an image.

## Release checklist for image distributors

1. Build each architecture; extract and archive its actual runtime inventory.
2. Retrieve and retain the exact corresponding Debian source packages identified
   by the source-package/version columns, including patches and build material
   required by their licenses. Debian source indexes and snapshot.debian.org can
   help locate versions; do not substitute the latest upstream release.
3. Publish the required corresponding-source artifacts alongside the image release,
   with license notices and recipient instructions, using an applicable license
   compliance mechanism. Include other covered runtime libraries, not just ffmpeg.
4. Verify source hashes, notices and architecture-specific inventories against
   the image digests before calling the release cleared for redistribution.

The source-bundling gate applies to newly built images, not retroactively to
previous releases. Architecture-specific image inspection remains required.
