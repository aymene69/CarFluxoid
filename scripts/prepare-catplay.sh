#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
git -C "$root" submodule update --init --recursive
if git -C "$root/catplay" apply --check "$root/patches/catplay.patch" 2>/dev/null; then
 git -C "$root/catplay" apply "$root/patches/catplay.patch"
elif git -C "$root/catplay" apply --reverse --check "$root/patches/catplay.patch" 2>/dev/null; then
 echo 'CatPlay patch already applied.'
else
 echo 'CatPlay differs from the expected revision; inspect local changes before applying the patch.' >&2
 exit 1
fi
