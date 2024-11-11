use gdal::raster;
use godot::prelude::*;
use std::{borrow, cmp, collections, ops, path};

/// Error message as either `&'static str` or `String`.
pub type Error = borrow::Cow<'static, str>;

pub enum ZipInfo {
  /// Chart raster data.
  Chart(Vec<path::PathBuf>),

  /// NASR aeronautical data.
  #[allow(unused)]
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
  pub fn _contains(&self, coord: Coord) -> bool {
    coord.x >= self.min.x && coord.x < self.max.x && coord.y >= self.min.y && coord.y < self.max.y
  }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Pos {
  pub x: i32,
  pub y: i32,
}

impl From<(i32, i32)> for Pos {
  fn from((x, y): (i32, i32)) -> Self {
    Self { x, y }
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
  pub fn is_valid(&self) -> bool {
    self.w > 0 && self.h > 0
  }

  pub fn contains(&self, coord: Coord) -> bool {
    let w = self.w as f64;
    let h = self.h as f64;
    coord.x >= 0.0 && coord.x < w && coord.y >= 0.0 && coord.y < h
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

pub struct ImageData {
  pub w: usize,
  pub h: usize,
  pub px: Vec<[u8; 4]>,
}

/// Check if a GDAL color will fit into a `Color`.
pub fn check_color(color: raster::RgbaEntry) -> bool {
  const COMP_RANGE: ops::Range<i16> = 0..256;
  COMP_RANGE.contains(&color.r)
    && COMP_RANGE.contains(&color.g)
    && COMP_RANGE.contains(&color.b)
    && COMP_RANGE.contains(&color.a)
}

/// Convert a GDAL color to `[u8; 4]`.
pub fn color(color: &raster::RgbaEntry) -> [u8; 4] {
  [color.r as u8, color.g as u8, color.b as u8, color.a as u8]
}

/// Convert a GDAL color to `[u8; 4]` and invert the luminance.
pub fn inverted_color(color: &raster::RgbaEntry) -> [u8; 4] {
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
