use gdal::{raster, spatial_ref};
use godot::prelude::*;
use std::{borrow, cmp, collections, ops, path};

#[allow(unused)]
pub fn print_children(nodes: Array<Gd<Node>>) {
  for child in nodes.iter_shared() {
    let name = child.get_name();
    godot_print!("{name}");
    print_children(child.get_children());
  }
}

pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

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

pub trait ToI32 {
  fn to_i32(self) -> Option<i32>;
}

impl ToI32 for i64 {
  fn to_i32(self) -> Option<i32> {
    let cast = self as i32;
    if cast as i64 == self {
      return Some(cast);
    }
    None
  }
}

pub trait ToU32 {
  fn to_u32(self) -> Option<u32>;
}

impl ToU32 for i64 {
  fn to_u32(self) -> Option<u32> {
    let cast = self as u32;
    if cast as i64 == self {
      return Some(cast);
    }
    None
  }
}

#[derive(Default, Eq, PartialEq)]
pub struct WinInfo {
  pub pos: Option<Pos>,
  pub size: Option<Size>,
  pub maxed: bool,
}

impl WinInfo {
  #[allow(unused)]
  pub fn from_value(value: Option<&serde_json::Value>) -> Self {
    if let Some(value) = value {
      let pos = value.get(WinInfo::POS_KEY).and_then(Pos::from_value);
      let size = value.get(WinInfo::SIZE_KEY).and_then(Size::from_value);
      let maxed = value
        .get(WinInfo::MAXED_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
      return Self { pos, size, maxed };
    }
    WinInfo::default()
  }

  #[allow(unused)]
  pub fn to_value(&self) -> serde_json::Value {
    let mut value = serde_json::json!({});

    if let Some(pos) = &self.pos {
      value[WinInfo::POS_KEY] = pos.to_value();
    }

    if let Some(size) = &self.size {
      value[WinInfo::SIZE_KEY] = size.to_value();
    }

    value[WinInfo::MAXED_KEY] = serde_json::Value::Bool(self.maxed);
    value
  }

  const POS_KEY: &'static str = "pos";
  const SIZE_KEY: &'static str = "size";
  const MAXED_KEY: &'static str = "maxed";
}

pub trait Transform {
  fn transform(&self, coord: Coord) -> Result<Coord, gdal::errors::GdalError>;
}

impl Transform for spatial_ref::CoordTransform {
  fn transform(&self, coord: Coord) -> Result<Coord, gdal::errors::GdalError> {
    let mut x = [coord.x];
    let mut y = [coord.y];
    self.transform_coords(&mut x, &mut y, &mut [])?;
    Ok(Coord { x: x[0], y: y[0] })
  }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coord {
  pub x: f64,
  pub y: f64,
}

impl From<(f64, f64)> for Coord {
  fn from((x, y): (f64, f64)) -> Self {
    Self { x, y }
  }
}

impl ops::Mul<f64> for Coord {
  type Output = Self;

  fn mul(self, scale: f64) -> Self {
    Self {
      x: self.x * scale,
      y: self.y * scale,
    }
  }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bounds {
  pub min: Coord,
  pub max: Coord,
}

impl Bounds {
  pub fn contains(&self, coord: Coord) -> bool {
    coord.x >= self.min.x && coord.x < self.max.x && coord.y >= self.min.y && coord.y < self.max.y
  }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Pos {
  pub x: i32,
  pub y: i32,
}

impl Pos {
  pub fn from_value(value: &serde_json::Value) -> Option<Self> {
    let x = value.get(0)?.as_i64()?.to_i32()?;
    let y = value.get(1)?.as_i64()?.to_i32()?;
    Some(Self { x, y })
  }

  pub fn to_value(self) -> serde_json::Value {
    serde_json::json!([self.x, self.y])
  }
}

impl ops::Sub for Pos {
  type Output = Self;

  fn sub(mut self, offset: Pos) -> Self {
    self.x -= offset.x;
    self.y -= offset.y;
    self
  }
}

impl From<(i32, i32)> for Pos {
  fn from((x, y): (i32, i32)) -> Self {
    Self { x, y }
  }
}

impl From<(f32, f32)> for Pos {
  fn from((x, y): (f32, f32)) -> Self {
    Self {
      x: x as i32,
      y: y as i32,
    }
  }
}

impl From<Vector2> for Pos {
  fn from(pos: Vector2) -> Self {
    Self {
      x: pos.x as i32,
      y: pos.y as i32,
    }
  }
}

impl From<Pos> for Vector2 {
  fn from(pos: Pos) -> Self {
    Self {
      x: pos.x as f32,
      y: pos.y as f32,
    }
  }
}

impl From<Pos> for (isize, isize) {
  fn from(pos: Pos) -> (isize, isize) {
    (pos.x as isize, pos.y as isize)
  }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Size {
  pub w: u32,
  pub h: u32,
}

impl Size {
  pub fn from_value(value: &serde_json::Value) -> Option<Self> {
    let w = value.get(0)?.as_i64()?.to_u32()?;
    let h = value.get(1)?.as_i64()?.to_u32()?;
    Some(Self { w, h })
  }

  pub fn to_value(self) -> serde_json::Value {
    serde_json::json!([self.w, self.h])
  }

  pub fn is_valid(&self) -> bool {
    self.w > 0 && self.h > 0
  }

  pub fn contains(&self, coord: Coord) -> bool {
    let w = self.w as f64;
    let h = self.h as f64;
    coord.x >= 0.0 && coord.x < w && coord.y >= 0.0 && coord.y < h
  }
}

impl ops::Mul<f64> for Size {
  type Output = Self;

  fn mul(self, scale: f64) -> Self {
    Self {
      w: (self.w as f64 * scale).round() as u32,
      h: (self.h as f64 * scale).round() as u32,
    }
  }
}

impl From<(u32, u32)> for Size {
  fn from((w, h): (u32, u32)) -> Self {
    Self { w, h }
  }
}

impl From<(usize, usize)> for Size {
  fn from((w, h): (usize, usize)) -> Self {
    Self {
      w: w as u32,
      h: h as u32,
    }
  }
}

impl From<Vector2> for Size {
  fn from(size: Vector2) -> Self {
    Self {
      w: size.x.round() as u32,
      h: size.y.round() as u32,
    }
  }
}

impl From<Size> for Vector2 {
  fn from(size: Size) -> Self {
    Self {
      x: size.w as f32,
      y: size.h as f32,
    }
  }
}

impl From<Size> for (usize, usize) {
  fn from(size: Size) -> (usize, usize) {
    (size.w as usize, size.h as usize)
  }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rect {
  pub pos: Pos,
  pub size: Size,
}

impl Rect {
  pub fn scaled(&self, scale: f32) -> Self {
    Self {
      pos: Pos {
        x: (self.pos.x as f32 * scale) as i32,
        y: (self.pos.y as f32 * scale) as i32,
      },
      size: Size {
        w: (self.size.w as f32 * scale).round() as u32,
        h: (self.size.h as f32 * scale).round() as u32,
      },
    }
  }

  pub fn fitted(&self, size: Size) -> Self {
    let x = if self.pos.x < 0 {
      0
    } else if self.pos.x as u32 + self.size.w > size.w {
      cmp::max(0, size.w as i32 - self.size.w as i32)
    } else {
      self.pos.x
    };

    let w = if (x as u32 + self.size.w) > size.w {
      size.w - x as u32
    } else {
      self.size.w
    };

    let y = if self.pos.y < 0 {
      0
    } else if self.pos.y as u32 + self.size.h > size.h {
      cmp::max(0, size.h as i32 - self.size.h as i32)
    } else {
      self.pos.y
    };

    let h = if (y as u32 + self.size.h) > size.h {
      size.h - y as u32
    } else {
      self.size.h
    };

    Self {
      pos: Pos { x, y },
      size: Size { w, h },
    }
  }
}

impl From<Rect2> for Rect {
  fn from(rect: Rect2) -> Self {
    Self {
      pos: rect.position.into(),
      size: rect.size.into(),
    }
  }
}

impl From<Rect> for Rect2 {
  fn from(rect: Rect) -> Self {
    Self {
      position: rect.pos.into(),
      size: rect.size.into(),
    }
  }
}

const HASHABLE32_SCALE: f32 = (1 << 23) as f32;

/// Represents a f32 in the 0..=1 range as a hashable value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Hashable(u32);

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

impl From<f64> for Hashable {
  fn from(val: f64) -> Self {
    assert!((0.0..=1.0).contains(&val));
    Hashable((val as f32 * HASHABLE32_SCALE) as u32)
  }
}

impl From<Hashable> for f32 {
  fn from(hashable: Hashable) -> Self {
    hashable.value()
  }
}

impl From<Hashable> for f64 {
  fn from(hashable: Hashable) -> Self {
    hashable.value() as f64
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

#[allow(unused)]
/// Return the file stem portion of a path as a `String`.
pub fn stem_string<P: AsRef<path::Path>>(path: P) -> Option<String> {
  stem_str(path.as_ref()).map(|stem| stem.to_owned())
}

/// Return the file stem portion of a path as a `&str`.
pub fn stem_str(path: &path::Path) -> Option<&str> {
  path.file_stem()?.to_str()
}

/// Return the folder of a path as a `String`.
pub fn folder_string<P: AsRef<path::Path>>(path: P) -> Option<String> {
  folder_str(path.as_ref()).map(|stem| stem.to_owned())
}

/// Return the folder of a path as a `&str`.
pub fn folder_str(path: &path::Path) -> Option<&str> {
  path.parent()?.to_str()
}
