#!/bin/sh

set -eu

BUSYBOX_URL='https://github.com/docker-library/busybox/raw/31d342ad033e27c18723a516a2274ab39547be27/stable/glibc/busybox.tar.xz'

mkdir -p rootfs && curl -fsSL --retry 5 $BUSYBOX_URL | tar -xJ --exclude './dev/*' -C rootfs
