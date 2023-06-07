#!/bin/sh
# Deterministic board-local artifacts; never inherit another project's target dir.
set -eu
cd "$(dirname "$0")/.."
case "${1:-rpi3}" in
    rpi3) board=rpi3; cpu=cortex-a53 ;;
    rpi4) board=rpi4; cpu=cortex-a72 ;;
    *) printf 'usage: %s [rpi3|rpi4]\n' "$0" >&2; exit 2 ;;
esac
if [ "$#" -gt 1 ]; then printf 'only one board argument is accepted\n' >&2; exit 2; fi
export CARGO_TARGET_DIR="$PWD/target/$board"
export CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
export CARGO_UNSTABLE_SPARSE_REGISTRY=true
export LD_SCRIPT_PATH="$PWD/src/bsp/raspberrypi"
export RUSTFLAGS="-D warnings -C target-cpu=$cpu -C link-arg=--library-path=$LD_SCRIPT_PATH -C link-arg=--script=kernel.ld"
cargo build --locked --release --bin kernel --target aarch64-unknown-none-softfloat --no-default-features --features "bsp_$board"
if [ -n "${OBJCOPY:-}" ]; then
    objcopy=$OBJCOPY
elif command -v aarch64-none-elf-objcopy >/dev/null 2>&1; then
    objcopy=aarch64-none-elf-objcopy
elif command -v llvm-objcopy >/dev/null 2>&1; then
    objcopy=llvm-objcopy
else
    objcopy=rust-objcopy
fi
"$objcopy" -O binary --strip-all "$CARGO_TARGET_DIR/aarch64-unknown-none-softfloat/release/kernel" "$CARGO_TARGET_DIR/kernel8.img"
printf '%s\n' "$CARGO_TARGET_DIR/kernel8.img"
