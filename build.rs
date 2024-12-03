use std::path::Path;

fn main() {
  // This script is only needed for static builds.
  let Some(build_static) = option_env!("GDAL_STATIC") else {
    return;
  };

  if build_static != "1" {
    return;
  }

  // Make sure these are set.
  if cfg!(target_os = "android") {
    #[allow(clippy::option_env_unwrap)]
    option_env!("ANDROID_NDK_HOME").expect("ANDROID_NDK_HOME not set");
  }
  #[allow(clippy::option_env_unwrap)]
  option_env!("GDAL_VERSION").expect("GDAL_VERSION not set");

  // Get the GDAL_HOME path.
  #[allow(clippy::option_env_unwrap)]
  let gdal_home = option_env!("GDAL_HOME").expect("GDAL_HOME not set");

  // Set the library search path.
  let lib_path = Path::new(gdal_home).join("lib");
  println!("cargo::rustc-link-search={lib_path:?}");

  // Libraries to link against.
  let libs = [
    "crypto",
    "curl",
    "geotiff",
    "json-c",
    "lzma",
    "proj",
    "sqlite3",
    "ssl",
    "stdc++",
    "tiff",
    "turbojpeg",
    "z",
  ];
  for lib in libs {
    println!("cargo::rustc-link-lib={lib}");
  }
}
