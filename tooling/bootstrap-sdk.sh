#!/bin/sh
set -eu
jobs=${JOBS:-4}
mkdir -p /build/aa /build/sdk-sources
python3 /input/tooling/fetch.py
python3 - <<'PY'
from pathlib import Path
import tarfile,json,subprocess
lock=json.loads(Path('/input/tooling/sources.lock.json').read_text())
for key,dest in [('linux','/build/linux'),('firmware_sources','/build/firmware-sources'),('musl','/build/aa/musl-1.2.5'),('mbedtls','/build/aa/mbedtls-rv'),('openssl','/build/sdk-sources/openssl-src')]:
 p=Path(dest)
 if p.exists():continue
 p.mkdir(parents=True)
 f='/input/local/downloads/'+lock['sources'][key]['file']
 subprocess.run(['tar','-xf',f,'--strip-components=1','-C',dest],check=True)
PY
if [ ! -f /build/aa/sysroot/include/stdio.h ]; then
 cd /build/aa/musl-1.2.5
 CC='clang-19 --target=riscv32-linux-musl -march=rv32gc -mabi=ilp32d' ./configure --target=riscv32-linux-musl --prefix=/build/aa/sysroot
 make -j"$jobs" install-headers obj/crt/crt1.o obj/crt/crti.o obj/crt/crtn.o
 make -C /build/linux ARCH=riscv headers_install INSTALL_HDR_PATH=/build/aa/sysroot
fi
mkdir -p /build/aa/bin
cat > /build/aa/bin/rv32-cc <<'CC'
#!/bin/sh
exec clang-19 --target=riscv32-linux-musl -march=rv32gc -mabi=ilp32d --sysroot=/build/aa/sysroot -isystem /build/aa/sysroot/include "$@"
CC
chmod +x /build/aa/bin/rv32-cc
if [ ! -f /build/aa/mbedtls-rv/library/libmbedtls.a ]; then
 make -C /build/aa/mbedtls-rv -j"$jobs" lib CC=/build/aa/bin/rv32-cc AR=llvm-ar-19 CFLAGS='-Os -fPIC'
fi
if [ ! -f /build/aa/openssl-install/lib/libcrypto.a ]; then
 cd /build/sdk-sources/openssl-src/openssl
 CC=/build/aa/bin/rv32-cc AR=llvm-ar-19 RANLIB=llvm-ranlib-19 perl Configure linux-generic32 no-shared no-tests no-asm no-legacy no-apps no-docs -DBROKEN_CLANG_ATOMICS --prefix=/build/aa/openssl-install
 make -j"$jobs"
 make install_sw
fi
if [ ! -f /build/linux/include/generated/autoconf.h ]; then
 python3 /input/tooling/prepare-kernel.py
 make -C /build/linux -j"$jobs" ARCH=riscv LLVM=-19 CROSS_COMPILE=riscv32-linux-gnu- olddefconfig modules_prepare
 cp /input/configs/beta-exports.symvers /build/linux/Module.symvers
fi
printf 'SDK ready: RV32 musl headers/CRT, MbedTLS, OpenSSL and pinned kernel module headers.\n'
