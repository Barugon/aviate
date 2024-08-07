use eframe::{egui, emath, epaint};
use gdal::{raster, spatial_ref};
use std::{borrow, cmp, collections, ops, path};

pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
pub const APP_ICON: &[u8] = include_bytes!("../res/icon.png");

#[macro_export]
macro_rules! debugln {
  ($($arg:tt)*) => (#[cfg(debug_assertions)] println!($($arg)*));
}

/// Return from function (and print error) if `Result` is not `Ok`.
#[macro_export]
macro_rules! ok {
  ($res:expr) => {
    match $res {
      Ok(val) => val,
      Err(err) => {
        println!("{err:?}");
        return;
      }
    }
  };
  ($res:expr, $ret:expr) => {
    match $res {
      Ok(val) => val,
      Err(err) => {
        println!("{err:?}");
        return $ret;
      }
    }
  };
}

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

  match gdal::vsi::read_dir(path, true) {
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
      return Err("Unable to read zip file".into());
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
  pub fn new(info: &egui::ViewportInfo) -> Self {
    Self {
      pos: info.inner_rect.map(|r| r.min.into()),
      size: info.inner_rect.map(|r| r.size().into()),
      maxed: info.fullscreen.unwrap_or(false),
    }
  }

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
  fn from(coord: (f64, f64)) -> Self {
    let (x, y) = coord;
    Self { x, y }
  }
}

impl From<emath::Pos2> for Coord {
  fn from(pos: emath::Pos2) -> Self {
    Self {
      x: pos.x as f64,
      y: pos.y as f64,
    }
  }
}

impl From<Coord> for emath::Pos2 {
  fn from(coord: Coord) -> Self {
    Self {
      x: coord.x as f32,
      y: coord.y as f32,
    }
  }
}

impl From<emath::Vec2> for Coord {
  fn from(pos: emath::Vec2) -> Self {
    Self {
      x: pos.x as f64,
      y: pos.y as f64,
    }
  }
}

impl ops::Mul<f64> for Coord {
  type Output = Coord;

  fn mul(self, scale: f64) -> Self::Output {
    Coord {
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

impl From<emath::Vec2> for Pos {
  fn from(pos: emath::Vec2) -> Self {
    Self {
      x: pos.x as i32,
      y: pos.y as i32,
    }
  }
}

impl From<emath::Pos2> for Pos {
  fn from(pos: emath::Pos2) -> Self {
    Self {
      x: pos.x as i32,
      y: pos.y as i32,
    }
  }
}

impl From<Pos> for emath::Pos2 {
  fn from(pos: Pos) -> Self {
    Self {
      x: pos.x as f32,
      y: pos.y as f32,
    }
  }
}

impl From<Pos> for (isize, isize) {
  fn from(pos: Pos) -> Self {
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

impl From<emath::Vec2> for Size {
  fn from(size: emath::Vec2) -> Self {
    Self {
      w: size.x.round() as u32,
      h: size.y.round() as u32,
    }
  }
}

impl From<(usize, usize)> for Size {
  fn from((x, y): (usize, usize)) -> Self {
    Self {
      w: x as u32,
      h: y as u32,
    }
  }
}

impl From<Size> for emath::Vec2 {
  fn from(size: Size) -> Self {
    Self {
      x: size.w as f32,
      y: size.h as f32,
    }
  }
}

impl From<Size> for (usize, usize) {
  fn from(size: Size) -> Self {
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
    let max_x = self.pos.x as u32 + self.size.w;
    let x = if self.pos.x < 0 {
      0
    } else if max_x > size.w {
      let d = (max_x - size.w) as i32;
      cmp::max(0, self.pos.x - d)
    } else {
      self.pos.x
    };

    let w = if max_x > size.w {
      size.w - self.pos.x as u32
    } else {
      self.size.w
    };

    let max_y = self.pos.y as u32 + self.size.h;
    let y = if self.pos.y < 0 {
      0
    } else if max_y > size.h {
      let d = (max_y - size.h) as i32;
      cmp::max(0, self.pos.y - d)
    } else {
      self.pos.y
    };

    let h = if max_y > size.h {
      size.h - self.pos.y as u32
    } else {
      self.size.h
    };

    Self {
      pos: Pos { x, y },
      size: Size { w, h },
    }
  }
}

impl From<Rect> for emath::Rect {
  fn from(rect: Rect) -> Self {
    Self::from_min_size(rect.pos.into(), rect.size.into())
  }
}

const HASHABLE32_SCALE: f32 = (1 << 23) as f32;

/// Represents a f32 in the 0..=1 range as a hashable value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Hashable(u32);

impl Hashable {
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
    hashable.0 as f32 / HASHABLE32_SCALE
  }
}

impl From<Hashable> for f64 {
  fn from(hashable: Hashable) -> Self {
    (hashable.0 as f32 / HASHABLE32_SCALE) as f64
  }
}

pub fn scale_rect(rect: emath::Rect, scale: f32) -> emath::Rect {
  emath::Rect {
    min: emath::Pos2 {
      x: rect.min.x * scale,
      y: rect.min.y * scale,
    },
    max: emath::Pos2 {
      x: rect.max.x * scale,
      y: rect.max.y * scale,
    },
  }
}

/// Return the file stem portion of a path as a `String`.
pub fn stem_string<P: AsRef<path::Path>>(path: P) -> Option<String> {
  stem_str(path.as_ref()).map(|stem| stem.to_owned())
}

/// Return the file stem portion of a path as a `&str`.
pub fn stem_str(path: &path::Path) -> Option<&str> {
  path.file_stem()?.to_str()
}

/// Returns the text with each word capitalized.
pub fn title_case(text: &str) -> String {
  let mut first = true;
  let mut result = String::new();
  for ch in text.chars() {
    if ch.is_whitespace() {
      first = true;
      result.push(ch);
    } else if first {
      first = false;
      for uch in ch.to_uppercase() {
        result.push(uch);
      }
    } else {
      result.push(ch);
    }
  }
  result
}

/// Convert degrees, minutes, seconds to decimal degrees.
#[allow(unused)]
pub fn to_dec_deg(deg: f64, min: f64, sec: f64) -> Option<f64> {
  if min >= 0.0 && sec >= 0.0 {
    const DEG_PER_MIN: f64 = 1.0 / 60.0;
    const DEG_PER_SEC: f64 = DEG_PER_MIN / 60.0;
    return Some(deg.signum() * (deg.abs() + min * DEG_PER_MIN + sec * DEG_PER_SEC));
  }
  None
}

/// Convert a decimal degree angle to +/- deg, min, sec.
pub fn to_deg_min_sec(dd: f64) -> (f64, f64, f64) {
  let sign = dd.signum();
  let dd = dd.abs();
  let deg = dd.trunc();
  let dm = (dd - deg) * 60.0;
  let min = dm.trunc();
  let sec = (dm - min) * 60.0;
  (sign * deg, min, sec)
}

/// Nicely format a degrees, minutes, seconds string from latitude in decimal degrees.
pub fn format_lat(dd: f64) -> Option<String> {
  if (-90.0..=90.0).contains(&dd) {
    let (deg, min, sec) = to_deg_min_sec(dd);
    let sec = (sec * 100.0).round() as u32;
    let frac = sec % 100;
    let sec = sec / 100;
    let sn = if deg < 0.0 { 'S' } else { 'N' };
    let deg = deg.abs();
    return Some(format!("{deg:02}°{min:02}'{sec:02}.{frac:02}\"{sn}"));
  }
  None
}

/// Nicely format a degrees, minutes, seconds string from longitude in decimal degrees.
pub fn format_lon(dd: f64) -> Option<String> {
  if (-180.0..=180.0).contains(&dd) {
    let (deg, min, sec) = to_deg_min_sec(dd);
    let sec = (sec * 100.0).round() as u32;
    let frac = sec % 100;
    let sec = sec / 100;
    let we = if deg < 0.0 { 'W' } else { 'E' };
    let deg = deg.abs();
    return Some(format!("{deg:03}°{min:02}'{sec:02}.{frac:02}\"{we}"));
  }
  None
}

/// Check if a GDAL color will fit into an egui color.
pub fn check_color(color: raster::RgbaEntry) -> bool {
  const COMP_RANGE: ops::Range<i16> = 0..256;
  COMP_RANGE.contains(&color.r)
    && COMP_RANGE.contains(&color.g)
    && COMP_RANGE.contains(&color.b)
    && COMP_RANGE.contains(&color.a)
}

/// Convert a GDAL color to an egui color.
pub fn color(color: &raster::RgbaEntry) -> epaint::Color32 {
  epaint::Color32::from_rgba_unmultiplied(
    color.r as u8,
    color.g as u8,
    color.b as u8,
    color.a as u8,
  )
}

/// Convert a GDAL color to an egui color and invert the luminance.
pub fn inverted_color(color: &raster::RgbaEntry) -> epaint::Color32 {
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

  epaint::Color32::from_rgba_unmultiplied(r, g, b, color.a as u8)
}

mod test {
  #[test]
  fn test_dd_lat_lon_conversion() {
    let dd = super::to_dec_deg(0.0, 59.0, 60.0).unwrap();
    assert!(dd == 1.0);

    let dd = super::to_dec_deg(-0.0, 59.0, 60.0).unwrap();
    assert!(dd == -1.0);

    let dd = super::to_dec_deg(34.0, 5.0, 6.9).unwrap();
    let lat = super::format_lat(dd).unwrap();
    assert!(lat == "34°05'06.90\"N");

    let dd = super::to_dec_deg(-26.0, 15.0, 44.63).unwrap();
    let lat = super::format_lat(dd).unwrap();
    assert!(lat == "26°15'44.63\"S");

    let dd = super::to_dec_deg(22.0, 24.0, 3.03).unwrap();
    let lon = super::format_lon(dd).unwrap();
    assert!(lon == "022°24'03.03\"E");

    let dd = super::to_dec_deg(-117.0, 8.0, 47.0).unwrap();
    let lon = super::format_lon(dd).unwrap();
    assert!(lon == "117°08'47.00\"W");
  }

  #[test]
  fn test_title_case() {
    assert!(super::title_case("title case text") == "Title Case Text");
  }

  #[test]
  fn test_i64_conversion() {
    use super::ToI32;
    use super::ToU32;

    let val = 0 as i64;
    assert!(val.to_i32().is_some());
    assert!(val.to_u32().is_some());

    let val = 1 as i64;
    assert!(val.to_i32().is_some());
    assert!(val.to_u32().is_some());

    let val = -1 as i64;
    assert!(val.to_i32().is_some());
    assert!(val.to_u32().is_none());

    let val = i32::MAX as i64;
    assert!(val.to_i32().is_some());
    assert!(val.to_u32().is_some());

    let val = i32::MAX as i64 + 1;
    assert!(val.to_i32().is_none());
    assert!(val.to_u32().is_some());

    let val = i32::MIN as i64;
    assert!(val.to_i32().is_some());
    assert!(val.to_u32().is_none());

    let val = i32::MIN as i64 - 1;
    assert!(val.to_i32().is_none());
    assert!(val.to_u32().is_none());

    let val = u32::MAX as i64;
    assert!(val.to_i32().is_none());
    assert!(val.to_u32().is_some());

    let val = u32::MAX as i64 + 1;
    assert!(val.to_i32().is_none());
    assert!(val.to_u32().is_none());

    let val = -(u32::MAX as i64);
    assert!(val.to_i32().is_none());
    assert!(val.to_u32().is_none());
  }
}
