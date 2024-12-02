use std::path::Path;

fn main() {
  let Some(build_static) = option_env!("GDAL_STATIC") else {
    return;
  };

  if build_static != "1" {
    return;
  }

  if cfg!(target_os = "android") {
    #[allow(clippy::option_env_unwrap)]
    option_env!("ANDROID_NDK_HOME").expect("ANDROID_NDK_HOME not set");
  }
  #[allow(clippy::option_env_unwrap)]
  option_env!("GDAL_VERSION").expect("GDAL_VERSION not set");
  #[allow(clippy::option_env_unwrap)]
  let gdal_home = option_env!("GDAL_HOME").expect("GDAL_HOME not set");

  let lib_path = Path::new(gdal_home).join("lib");
  println!("cargo::rustc-link-search={lib_path:?}");

  let libs = [
    "stdc++",
    "z",
    "json-c",
    "proj",
    "sqlite3",
    "curl",
    "crypto",
    "tiff",
    "turbojpeg",
    "lzma",
    "ssl",
    "geotiff",
  ];
  for lib in libs {
    println!("cargo::rustc-link-lib={lib}");
  }
}
