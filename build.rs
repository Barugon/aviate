fn main() {
  // This script is only needed for GDAL static builds.
  if option_env!("GDAL_STATIC").unwrap_or("0") != "1" {
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
