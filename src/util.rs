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
  _get_zip_info(path.as_ref())
}

fn _get_zip_info(path: &path::Path) -> Result<ZipInfo, Error> {
  let Some(path) = path.to_str() else {
    return Err("Invalid unicode in zip file path".into());
  };

  // Concatenate the VSI prefix.
  let path = ["/vsizip/", path].concat();

  match gdal::vsi::read_dir(&path, true) {
    Ok(files) => {
      let mut csv = path::PathBuf::new();
      let mut shp = path::PathBuf::new();
      let mut tfws = collections::HashSet::new();
      let mut tifs = Vec::new();
      for file in files {
        let Some(ext) = file.extension() else {
          continue;
        };

        // Make sure there's no invalid unicode.
        if file.to_str().is_none() {
          continue;
        }

        if ext.eq_ignore_ascii_case("tfw") {
          tfws.insert(file);
        } else if ext.eq_ignore_ascii_case("tif") {
          tifs.push(file);
        } else if csv.as_os_str().is_empty() && ext.eq_ignore_ascii_case("zip") {
          if let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) {
            if stem.to_ascii_uppercase().ends_with("_CSV") {
              // CSV zip file contained in the main zip.
              csv = file;
            }
          }
        } else if shp.as_os_str().is_empty() && ext.eq_ignore_ascii_case("shp") {
          if let Some(stem) = file.file_stem() {
            if stem.eq_ignore_ascii_case("Class_Airspace") {
              // Use the folder for shape files.
              if let Some(parent) = file.parent() {
                parent.clone_into(&mut shp);
              }
            }
          }
        }
      }

      // Both the shape folder and CSV zip must be present for aero data to be valid.
      if !csv.as_os_str().is_empty() && !shp.as_os_str().is_empty() {
        return Ok(ZipInfo::Aero { csv, shp });
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

/// Represents a f32 in the 0..=1 range as a hashable value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Hashable(u32);
const HASHABLE32_SCALE: f32 = (1 << 23) as f32;

impl Hashable {
  pub fn value(&self) -> f32 {
    self.0 as f32 / HASHABLE32_SCALE
  }

  pub fn inverse(&self) -> f32 {
    assert!(self.0 > 0);
    HASHABLE32_SCALE / self.0 as f32
  }
}

impl From<f32> for Hashable {
  fn from(val: f32) -> Self {
    assert!((0.0..=1.0).contains(&val));
    Hashable((val * HASHABLE32_SCALE) as u32)
  }
}

impl From<Hashable> for f32 {
  fn from(hashable: Hashable) -> Self {
    hashable.value()
  }
}

pub type Color = [u8; 4];

pub struct ImageData {
  pub w: usize,
  pub h: usize,
  pub px: Vec<Color>,
}

/// Check if a GDAL `RgbaEntry` will fit into a `Color`.
pub fn check_color(color: raster::RgbaEntry) -> bool {
  const COMP_RANGE: ops::Range<i16> = 0..256;
  COMP_RANGE.contains(&color.r)
    && COMP_RANGE.contains(&color.g)
    && COMP_RANGE.contains(&color.b)
    && COMP_RANGE.contains(&color.a)
}

/// Convert a GDAL `RgbaEntry` to a `Color`.
pub fn color(color: &raster::RgbaEntry) -> Color {
  [color.r as u8, color.g as u8, color.b as u8, color.a as u8]
}

/// Convert a GDAL `RgbaEntry` to a luminance inverted `Color`.
pub fn inverted_color(color: &raster::RgbaEntry) -> Color {
  let r = color.r as f32;
  let g = color.g as f32;
  let b = color.b as f32;

  // Convert to YCbCr and invert the luminance.
  let y = 255.0 - (r * 0.299 + g * 0.587 + b * 0.114);
  let cb = b * 0.5 - r * 0.168736 - g * 0.331264;
  let cr = r * 0.5 - g * 0.418688 - b * 0.081312;

  // Convert back to RGB.
  let r = (y + 1.402 * cr) as u8;
  let g = (y - 0.344136 * cb - 0.714136 * cr) as u8;
  let b = (y + 1.772 * cb) as u8;

  [r, g, b, color.a as u8]
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
