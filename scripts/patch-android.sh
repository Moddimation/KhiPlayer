#!/usr/bin/env bash
# Run this once *after* `cargo tauri android init` (and again any time that command has been
# re-run, e.g. a clean CI checkout) and *before* `cargo tauri android build`.
#
# `cargo tauri android init` only ever regenerates a fresh gen/android from the template; it
# doesn't know about MainActivity.kt's swipe-to-refresh / Ctrl+R additions, so those have to be
# re-applied as a small patch step instead of living directly in the generated tree.
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

GEN=src-tauri/gen/android
PACKAGE_DIR="$GEN/app/src/main/java/dev/local/khinsider"

if [ ! -d "$GEN" ]; then
  echo "::error::$GEN doesn't exist -- run 'cargo tauri android init' first."
  exit 1
fi
if [ ! -d "$PACKAGE_DIR" ]; then
  echo "::error::$PACKAGE_DIR not found. If the Tauri identifier in tauri.conf.json changed from"
  echo "dev.local.khinsider, update PACKAGE_DIR here (and the 'package' line in"
  echo "android-overlay/MainActivity.kt) to match."
  exit 1
fi

echo "Patching $PACKAGE_DIR/MainActivity.kt"
cp android-overlay/MainActivity.kt "$PACKAGE_DIR/MainActivity.kt"

GRADLE="$GEN/app/build.gradle.kts"
if ! grep -q "swiperefreshlayout" "$GRADLE"; then
  echo "Adding androidx.swiperefreshlayout dependency to $GRADLE"
  # Appends inside the existing `dependencies { ... }` block by inserting right after its opening
  # brace -- every Tauri-generated app/build.gradle.kts has exactly one such block.
  python3 - "$GRADLE" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
line = '    implementation("androidx.swiperefreshlayout:swiperefreshlayout:1.1.0")\n'
new_text, n = re.subn(r'(dependencies\s*\{\n)', r'\1' + line, text, count=1)
if n == 0:
    sys.exit("could not find a `dependencies {` block in " + path)
open(path, "w").write(new_text)
PY
else
  echo "$GRADLE already references swiperefreshlayout, skipping"
fi

echo "Android sources patched."
