#!/bin/sh
# Exercise the architecture-neutral kernel library without registry dependencies.
set -eu
cd "$(dirname "$0")/.."
mkdir -p target/host
rustc --edition=2021 --test src/lib.rs -D warnings -o target/host/kernel-tests
target/host/kernel-tests "$@"
