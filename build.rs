fn main() {
  // Temporary fix for warnings in rustc v1.84.0.
  // TODO: remove after next gdext release.
  println!("cargo::rustc-check-cfg=cfg(before_api, values(\"4.3\"))");

  // This script is only needed for GDAL static builds.
  if std::env::var("GDAL_STATIC").unwrap_or_default() != "1" {
    return;
  };

  // Use the correct C++ standard library.
  if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "android" {
    println!("cargo::rustc-link-lib=c++");
  } else {
    println!("cargo::rustc-link-lib=stdc++");
  }

  // GDAL dependencies.
  let libs = [
    "crypto",
    "curl",
    "geotiff",
    "geos",
    "geos_c",
    "json-c",
    "lzma",
    "proj",
    "sqlite3",
    "ssl",
    "tiff",
    "turbojpeg",
    "z",
  ];
  for lib in libs {
    println!("cargo::rustc-link-lib={lib}");
  }
}
