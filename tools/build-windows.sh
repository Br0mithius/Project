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
    z.write("target/x86_64-pc-windows-gnu/release/msh-deckbuilder.exe", "MSH-Deckbuilder/MSH Deckbuilder.exe")
    z.write("target/x86_64-pc-windows-gnu/release/msh-cli.exe", "MSH-Deckbuilder/msh-cli (command line).exe")
    z.write("data/cards.json", "MSH-Deckbuilder/data/cards.json")
    z.write("data/peer_field.txt", "MSH-Deckbuilder/data/peer_field.txt")
    # Shipped card power (measured across the league's decks); the user's own measurements
    # go in a separate file, so updating never overwrites them.
    import os
    if os.path.exists("data/sim_card_values_default.tsv"):
        z.write("data/sim_card_values_default.tsv", "MSH-Deckbuilder/data/sim_card_values_default.tsv")
    # No collection.txt: the app creates an empty one on first start, so unzipping over an
    # older copy never wipes a pool.
    z.write("dist/README-windows.txt", "MSH-Deckbuilder/README.txt")
    z.writestr("MSH-Deckbuilder/decks/", "")
print("wrote", out)
PY
