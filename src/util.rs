use crate::geom;
use gdal::raster;
use godot::{
  classes::{
    display_server::WindowMode, file_access::ModeFlags, os::SystemDir, DisplayServer, FileAccess,
    Os,
  },
  prelude::*,
};
use std::{borrow, cmp, collections, ops, path};

pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
pub const PROJ4_NAD83: &str = "+proj=longlat +datum=NAD83 +no_defs";
pub const ZOOM_RANGE: ops::RangeInclusive<f32> = 1.0 / 8.0..=1.0;

/// Error message as either `&'static str` or `String`.
pub type Error = borrow::Cow<'static, str>;

pub enum ZipInfo {
  /// Chart raster data.
  Chart(Vec<path::PathBuf>),

  /// NASR aeronautical data.
  Aero {
    csv: path::PathBuf,
    #[allow(unused)]
    shp: path::PathBuf,
  },
}

/// Returns information about what type of FAA data (if any) is contained in a zip file.
pub fn get_zip_info<P: AsRef<path::Path>>(path: P) -> Result<ZipInfo, Error> {
  |path: &path::Path| -> Result<ZipInfo, Error> {
    fn get_csv_path(path: &path::Path, folder: &path::Path) -> Option<path::PathBuf> {
      let files = gdal::vsi::read_dir(path.join(folder), false).ok()?;
      for file in files {
        let Some(ext) = file.extension() else {
          continue;
        };

        if ext.eq_ignore_ascii_case("zip") {
          let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
          };

          if stem.to_ascii_uppercase().ends_with("_CSV") {
            return Some(folder.join(file));
          }
        }
      }
      None
    }

    fn get_shp_path(path: &path::Path, folder: &path::Path) -> Option<path::PathBuf> {
      let files = gdal::vsi::read_dir(path.join(folder), false).ok()?;
      for file in files {
        let Some(name) = file.file_name() else {
          continue;
        };

        if name.eq_ignore_ascii_case("Shape_Files") {
          return get_shp_path(path, &folder.join(name));
        } else if let Some(stem) = path::Path::new(name).file_stem() {
          if stem.eq_ignore_ascii_case("Class_Airspace") {
            // Use the folder for shape files.
            return Some(folder.into());
          }
        }
      }
      None
    }

    let Some(path) = path.to_str() else {
      return Err("Invalid unicode in zip file path".into());
    };

    // Concatenate the VSI prefix.
    let path = ["/vsizip/", path].concat();
    let path = path::PathBuf::from(path);

    match gdal::vsi::read_dir(&path, false) {
      Ok(files) => {
        let mut csv = path::PathBuf::new();
        let mut shp = path::PathBuf::new();
        let mut tfws = collections::HashSet::new();
        let mut tifs = Vec::new();
        for file in files {
          if let Some(name) = file.file_name() {
            if name.eq_ignore_ascii_case("Additional_Data") {
              if let Some(shp_path) = get_shp_path(&path, path::Path::new(name)) {
                shp = shp_path;
              }
            } else if name.eq_ignore_ascii_case("CSV_Data") {
              if let Some(csv_path) = get_csv_path(&path, path::Path::new(name)) {
                csv = csv_path;
              }
            }

            if !csv.as_os_str().is_empty() && !shp.as_os_str().is_empty() {
              return Ok(ZipInfo::Aero { csv, shp });
            }
          }

          let Some(ext) = file.extension() else {
            continue;
          };

          if ext.eq_ignore_ascii_case("tfw") {
            tfws.insert(file);
          } else if ext.eq_ignore_ascii_case("tif") {
            tifs.push(file);
          }
        }

        // Only accept TIFF files that have matching TFW files.
        let mut files = Vec::with_capacity(cmp::min(tifs.len(), tfws.len()));
        for file in tifs {
          if tfws.contains(&file.with_extension("tfw")) {
            files.push(file);
          }
        }

        if !files.is_empty() {
          return Ok(ZipInfo::Chart(files));
        }
      }
      Err(_) => {
        let path = path::Path::new(&path);
        if let Some(ext) = path.extension() {
          if ext.eq_ignore_ascii_case("zip") {
            return Err("Unable to read zip file".into());
          }
        }
        return Err("Not a zip file".into());
      }
    }

    Err("Zip file does not contain usable data".into())
  }(path.as_ref())
}

#[derive(Default, Eq, PartialEq)]
pub struct WinInfo {
  pub pos: Option<geom::Pos>,
  pub size: Option<geom::Size>,
  pub maxed: bool,
}

impl WinInfo {
  pub fn from_display(display_server: &Gd<DisplayServer>) -> Self {
    let pos = Some(display_server.window_get_position().into());
    let size = Some(display_server.window_get_size().into());
    let maxed = display_server.window_get_mode() == WindowMode::MAXIMIZED;
    Self { pos, size, maxed }
  }

  pub fn from_variant(value: Option<Variant>) -> Self {
    if let Some(value) = value.and_then(|v| v.try_to::<Dictionary>().ok()) {
      let pos = value
        .get(WinInfo::POS_KEY)
        .and_then(geom::Pos::from_variant);
      let size = value
        .get(WinInfo::SIZE_KEY)
        .and_then(geom::Size::from_variant);
      let maxed = value
        .get(WinInfo::MAXED_KEY)
        .and_then(|v| v.try_to::<bool>().ok())
        .unwrap_or(false);
      return Self { pos, size, maxed };
    }
    WinInfo::default()
  }

  pub fn to_variant(&self) -> Variant {
    let mut dict = Dictionary::new();

    if let Some(pos) = &self.pos {
      dict.set(WinInfo::POS_KEY, pos.to_variant());
    }

    if let Some(size) = &self.size {
      dict.set(WinInfo::SIZE_KEY, size.to_variant());
    }

    dict.set(WinInfo::MAXED_KEY, Variant::from(self.maxed));
    Variant::from(dict)
  }

  const POS_KEY: &'static str = "pos";
  const SIZE_KEY: &'static str = "size";
  const MAXED_KEY: &'static str = "maxed";
}

pub trait ToI32 {
  fn to_i32(self) -> Option<i32>;
}

impl ToI32 for f64 {
  fn to_i32(self) -> Option<i32> {
    let cast = self as i32;
    if cast as Self == self {
      return Some(cast);
    }
    None
  }
}

impl ToI32 for Variant {
  fn to_i32(self) -> Option<i32> {
    self.try_to::<f64>().ok()?.to_i32()
  }
}

pub trait ToU32 {
  fn to_u32(self) -> Option<u32>;
}

impl ToU32 for f64 {
  fn to_u32(self) -> Option<u32> {
    let cast = self as u32;
    if cast as Self == self {
      return Some(cast);
    }
    None
  }
}

impl ToU32 for Variant {
  fn to_u32(self) -> Option<u32> {
    self.try_to::<f64>().ok()?.to_u32()
  }
}

pub struct ImageData {
  pub w: usize,
  pub h: usize,
  pub px: Vec<[u8; 4]>,
}

/// Check if a GDAL `RgbaEntry` will fit into a `[u8; 4]`.
pub fn check_color(color: raster::RgbaEntry) -> bool {
  const COMP_RANGE: ops::Range<i16> = 0..256;
  COMP_RANGE.contains(&color.r)
    && COMP_RANGE.contains(&color.g)
    && COMP_RANGE.contains(&color.b)
    && color.a == u8::MAX as i16
}

/// Convert a GDAL `RgbaEntry` to a `[f32; 3]`.
pub fn color_f32(color: &raster::RgbaEntry) -> [f32; 3] {
  const SCALE: f32 = 1.0 / u8::MAX as f32;
  [
    color.r as f32 * SCALE,
    color.g as f32 * SCALE,
    color.b as f32 * SCALE,
  ]
}

/// Convert a GDAL `RgbaEntry` to a luminance inverted `[f32; 3]`.
pub fn inverted_color_f32(color: &raster::RgbaEntry) -> [f32; 3] {
  let r = color.r as f32;
  let g = color.g as f32;
  let b = color.b as f32;

  // Convert to YCbCr and invert the luminance.
  let y = 255.0 - (r * 0.299 + g * 0.587 + b * 0.114);
  let cb = b * 0.5 - r * 0.168736 - g * 0.331264;
  let cr = r * 0.5 - g * 0.418688 - b * 0.081312;

  // Convert back to RGB.
  const SCALE: f32 = 1.0 / u8::MAX as f32;
  let r = (y + 1.402 * cr) * SCALE;
  let g = (y - 0.344136 * cb - 0.714136 * cr) * SCALE;
  let b = (y + 1.772 * cb) * SCALE;

  [r, g, b]
}

/// Convert a `[f32; 3]` color to `[u8; 4]`
pub fn color(color: [f32; 3]) -> [u8; 4] {
  const SCALE: f32 = u8::MAX as f32;
  [
    (color[0] * SCALE) as u8,
    (color[1] * SCALE) as u8,
    (color[2] * SCALE) as u8,
    u8::MAX,
  ]
}

/// Return the file stem portion of a path as a `&str`.
pub fn stem_str(path: &path::Path) -> Option<&str> {
  path.file_stem()?.to_str()
}

/// Return the folder of a path as a `String`.
pub fn folder_gstring<P: AsRef<path::Path>>(path: P) -> Option<GString> {
  folder_str(path.as_ref()).map(|stem| stem.into())
}

/// Return the folder of a path as a `&str`.
pub fn folder_str(path: &path::Path) -> Option<&str> {
  path.parent()?.to_str()
}

/// Get the OS specific downloads folder.
pub fn get_downloads_folder() -> GString {
  Os::singleton().get_system_dir(SystemDir::DOWNLOADS)
}

/// Load a text file.
pub fn load_text(path: &GString) -> Option<GString> {
  let file = FileAccess::open(path, ModeFlags::READ)?;
  Some(file.get_as_text())
}

/// Store a text file.
pub fn store_text(path: &GString, text: &GString) {
  let Some(mut file) = FileAccess::open(path, ModeFlags::WRITE) else {
    return;
  };

  file.store_string(text);
}

pub fn request_permissions() {
  godot::classes::Os::singleton().request_permissions();
}
