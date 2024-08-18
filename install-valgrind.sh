#!/usr/bin/env sh

# Script to install valgrind on a Debian-like OS

set -ex

VALGRIND_VER=${VALGRIND_VER:-3.23.0}
VALGRIND_BUILD_DIR=${VALGRIND_BUILD_DIR:-/tmp}

echo "Building valgrind ${VALGRIND_VER} in ${VALGRIND_BUILD_DIR}"

sudo apt-get update
sudo apt-get install -y --no-install-suggests --no-install-recommends \
  build-essential pkg-config

if [ ! -d "$VALGRIND_BUILD_DIR/valgrind-$VALGRIND_VER" ]; then
  cd "$VALGRIND_BUILD_DIR" || exit 1
  echo "Downloading valgrind sources..."
  curl -sSL "https://sourceware.org/pub/valgrind/valgrind-$VALGRIND_VER.tar.bz2" > valgrind-"$VALGRIND_VER".tar.bz2
  tar xf valgrind-"$VALGRIND_VER".tar.bz2
fi

cd "$VALGRIND_BUILD_DIR/valgrind-$VALGRIND_VER" || exit 1
echo "Building valgrind..."
./configure
make

echo "Installing valgrind..."
sudo make install
echo "Checking cachegrind version..."
valgrind --tool=cachegrind --version
