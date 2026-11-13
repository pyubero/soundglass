#!/bin/sh
# Builds soundglass_<version>_amd64.deb in the repository root.
# Build on the oldest distro to support: the package needs that distro's glibc or newer.
set -eu
cd "$(dirname "$0")"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
cargo build --release --locked

stage=target/deb
rm -rf "$stage"
install -Dsm755 target/release/soundglass "$stage/usr/bin/soundglass"
install -Dm644 soundglass.desktop "$stage/usr/share/applications/soundglass.desktop"
install -Dm644 icon/soundglass.svg "$stage/usr/share/icons/hicolor/scalable/apps/soundglass.svg"
install -Dm644 soundglass.metainfo.xml "$stage/usr/share/metainfo/io.github.pyubero.soundglass.metainfo.xml"
mkdir -p "$stage/usr/share/doc/soundglass" "$stage/DEBIAN"
./third-party-notices.py > "$stage/usr/share/doc/soundglass/copyright"
# libasound2 was renamed libasound2t64 in Ubuntu 24.04 and Debian 13.
# unzip unpacks the GPU libraries (Local AI); gdbus (libglib2.0-bin) reads the system theme.
cat > "$stage/DEBIAN/control" <<CONTROL
Package: soundglass
Version: $version
Section: sound
Priority: optional
Architecture: amd64
Installed-Size: $(du -sk "$stage/usr" | cut -f1)
Depends: libc6 (>= 2.35), libgcc-s1, libasound2 | libasound2t64, xdg-utils
Recommends: unzip, libglib2.0-bin
Maintainer: Pablo Yubero
Description: A small, fast audio player and quick editor
 Plays a folder without waiting, edits tags and makes quick cuts and fixes,
 never changing the original, with optional local AI tools and a practice mode.
CONTROL

dpkg-deb --build --root-owner-group "$stage" "soundglass_${version}_amd64.deb"
