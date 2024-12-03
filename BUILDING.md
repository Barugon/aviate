# Build Aviate

## GDAL shared

To build against a shared library of GDAL, just install `gdal-devel` and build.

```sh
sudo dnf install gdal-devel
```

> NOTE: That, of course, will not work for Android.

## GDAL static

### Build GDAL static libraries

- Prerequisites

```sh
git clone https://github.com/microsoft/vcpkg.git
cd vcpkg
./bootstrap-vcpkg.sh -disableMetrics
./vcpkg integrate install
```

- Build for Linux

```sh
export CC=clang
export CXX=clang++
./vcpkg install gdal[core]:x64-linux
```

- Build for Android

```sh
unset CC CXX
export ANDROID_NDK_HOME=<path/to/android_ndk_home>
./vcpkg install gdal[core]:arm64-android
```

### Build project with statically linked gdal

- Prerequisites

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk
```

- Build for Linux

```sh
GDAL_HOME=<path/to/vcpkg/gdal/stuff> GDAL_VERSION=<x.y.z> GDAL_STATIC=1 cargo build --target=x86_64-unknown-linux-gnu [--release]
```

- Build for Android

```sh
ANDROID_NDK_HOME=<path/to/android/ndk> GDAL_HOME=<path/to/vcpkg/gdal/stuff> GDAL_VERSION=<x.y.z> GDAL_STATIC=1 cargo ndk -t arm64-v8a build --target=aarch64-linux-android [--release]
```
