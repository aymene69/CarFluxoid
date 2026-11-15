#!/bin/sh
set -eu
/input/tooling/bootstrap-sdk.sh
python3 /input/tooling/prepare-base.py
export CARFLUXOID_SYSROOT=/build/firmware-root
sh /input/bridge-aa/build-v821.sh
sh /input/bridge-aa/build-carplay-rx.sh
mkdir -p /build/aa-bulk
cp /input/bridge-aa/kernel/aa_bulk.c /input/bridge-aa/kernel/aa_aoa.h /input/bridge-aa/kernel/Makefile /build/aa-bulk/
make -C /build/linux -j"${JOBS:-4}" ARCH=riscv LLVM=-19 CROSS_COMPILE=riscv32-linux-gnu- M=/build/aa-bulk modules
mkdir -p /input/local/artifacts
cp /build/aa/aa-phone-v821 /input/local/artifacts/
cp /build/aa/cp-rx-target/riscv32gc-unknown-linux-musl/release/catplay-aa-rx /input/local/artifacts/
llvm-strip-19 --strip-all /input/local/artifacts/catplay-aa-rx
cp /build/aa-bulk/aa_bulk.ko /input/local/artifacts/
llvm-objcopy-19 --dump-section .text=/build/new-module.text /input/local/artifacts/aa_bulk.ko
python3 - <<'PYTEST'
import json,hashlib
from pathlib import Path
expected=json.loads(Path('/input/tooling/sources.lock.json').read_text())['profile']['module_text_sha256']
actual=hashlib.sha256(Path('/build/new-module.text').read_bytes()).hexdigest()
if actual!=expected:raise RuntimeError('Gadget executable code differs from the pinned build reference')
print('USB gadget executable code matches the pinned build reference; see profile validation status.')
PYTEST
python3 /input/tooling/package.py
