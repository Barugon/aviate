# Build Aviate

## GDAL shared

To build against a shared library of GDAL, just install `gdal-devel` and build.

```sh
sudo dnf install gdal-devel
# -- or -- #
sudo apt install libgdal-dev
```

> NOTE: That, of course, will not work for Android.

## GDAL static

### Build GDAL static libraries

```sh
git clone https://github.com/microsoft/vcpkg.git
cd vcpkg
./bootstrap-vcpkg.sh # -disableMetrics
./vcpkg integrate install
```

- Build for Linux

```sh
export CC=clang
export CXX=clang++
./vcpkg install gdal[core,geos]:x64-linux
```

- Build for Android

```sh
export ANDROID_NDK_HOME=<path/to/android-ndk>
./vcpkg install gdal[core,geos]:arm64-android
```

### Build project with statically linked gdal

- Build for Linux

```sh
export GDAL_HOME=<path/to/vcpkg/installed/x64-linux>
export GDAL_VERSION=<x.y.z>
export GDAL_STATIC=1
cargo build --release
```

- Build for Android

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk
```

```sh
export ANDROID_NDK_HOME=<path/to/android-ndk>
export GDAL_HOME=<path/to/vcpkg/installed/arm64-android>
export GDAL_VERSION=<x.y.z>
export GDAL_STATIC=1
cargo ndk -t arm64-v8a build --target=aarch64-linux-android --release
```
