#!/bin/bash
# Збирає готовий пакет для Windows із Linux: тести -> exe -> dist/layout-switcher-windows.zip і dist/release/
# Потрібно: Rust із ціллю x86_64-pc-windows-gnu, gcc-mingw-w64-x86-64, python3.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}"

cargo test --lib
cargo build --release --target x86_64-pc-windows-gnu

OUT=dist/layout-switcher
rm -rf dist && mkdir -p "$OUT/dict"
cp target/x86_64-pc-windows-gnu/release/layout-switcher.exe "$OUT/"
cp dict/* "$OUT/dict/"
cp package/* "$OUT/"
cp LICENSE NOTICE.md "$OUT/"
python3 - <<'PY'
import shutil
shutil.make_archive("dist/layout-switcher-windows", "zip", "dist", "layout-switcher")
PY

# Те, що викладається в реліз на GitHub: окремий exe (з нього оновлюється програма), повний пакет і контрольні суми.
REL=dist/release
mkdir -p "$REL"
cp target/x86_64-pc-windows-gnu/release/layout-switcher.exe "$REL/"
cp dist/layout-switcher-windows.zip "$REL/"
( cd "$REL" && sha256sum layout-switcher.exe layout-switcher-windows.zip > SHA256SUMS && cat SHA256SUMS )
echo "готово: dist/layout-switcher-windows.zip, dist/release/ (exe, zip, SHA256SUMS)"
ls -la "$OUT"
