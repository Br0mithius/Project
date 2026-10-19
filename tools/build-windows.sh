#!/usr/bin/env bash
# Cross-compiles the Windows .exe from Linux and packages it as dist/MSH-Deckbuilder-windows.zip.
# Needs: rustup target add x86_64-pc-windows-gnu, and the mingw-w64 linker (apt install gcc-mingw-w64-x86-64).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --target x86_64-pc-windows-gnu
python3 - <<'PY'
import zipfile
out = "dist/MSH-Deckbuilder-windows.zip"
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.write("target/x86_64-pc-windows-gnu/release/msh-deckbuilder.exe", "MSH-Deckbuilder/msh-deckbuilder.exe")
    z.write("data/cards.json", "MSH-Deckbuilder/data/cards.json")
    z.write("data/collection.txt", "MSH-Deckbuilder/data/collection.txt")
    z.write("dist/README-windows.txt", "MSH-Deckbuilder/README.txt")
    z.writestr("MSH-Deckbuilder/decks/", "")
print("wrote", out)
PY
