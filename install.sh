#!/bin/sh
# Builds soundglass from this checkout, installs it for the current user and makes it the
# default app for the audio formats it plays.
# Undo: rm ~/.local/bin/soundglass ~/.local/share/applications/soundglass.desktop
# ~/.local/share/icons/hicolor/scalable/apps/soundglass.svg,
# then pick another default app in System Settings → Default Applications / File Associations.
set -eu
cd "$(dirname "$0")"

# The MimeType line of soundglass.desktop is the one list of formats claimed.
TYPES=$(sed -n 's/^MimeType=//p' soundglass.desktop | tr ';' ' ')

echo "Building (can take about a minute after code changes)…"
cargo build --release
install -Dm755 target/release/soundglass "$HOME/.local/bin/soundglass"
install -Dm644 icon/soundglass.svg "$HOME/.local/share/icons/hicolor/scalable/apps/soundglass.svg"
mkdir -p "$HOME/.local/share/applications"
# Absolute Exec path: ~/.local/bin isn't always on the PATH the desktop session uses.
sed "s|^Exec=soundglass|Exec=$HOME/.local/bin/soundglass|" soundglass.desktop > "$HOME/.local/share/applications/soundglass.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
# shellcheck disable=SC2086 # one argument per type, on purpose
xdg-mime default soundglass.desktop $TYPES

echo "Installed. Default for MP3 is now: $(xdg-mime query default audio/mpeg)"
