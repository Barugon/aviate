use gdal::spatial_ref;
use godot::prelude::*;
use std::{cmp, ops};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coord {
  pub x: f64,
  pub y: f64,
}

impl Coord {
  pub fn new(x: f64, y: f64) -> Self {
    Self { x, y }
  }

  pub fn from_variant(value: Variant) -> Option<Self> {
    let value = value.try_to::<Array<Variant>>().ok()?;
    let x = value.get(0)?.try_to::<f64>().ok()?;
    let y = value.get(1)?.try_to::<f64>().ok()?;
    Some(Self { x, y })
  }
}

impl From<Pos> for Coord {
  fn from(pos: Pos) -> Self {
    Self {
      x: pos.x as f64,
      y: pos.y as f64,
    }
  }
}

impl From<(f64, f64)> for Coord {
  fn from((x, y): (f64, f64)) -> Self {
    Self { x, y }
  }
}

impl From<Coord> for Vector2 {
  fn from(coord: Coord) -> Self {
    Self::new(coord.x as f32, coord.y as f32)
  }
}

impl ops::Sub<Coord> for Coord {
  type Output = Self;

  fn sub(self, coord: Coord) -> Self {
    Self {
      x: self.x - coord.x,
      y: self.y - coord.y,
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtentType {
  Contained,
  Exact,
}

pub struct Extent {
  pub xr: ops::RangeInclusive<f64>,
  pub yr: ops::RangeInclusive<f64>,
}

impl Extent {
  fn new(xr: ops::RangeInclusive<f64>, yr: ops::RangeInclusive<f64>) -> Self {
    Self { xr, yr }
  }

  /// Create an extent from a polygon. Also returns if the polygon is an exact rectangle or contained.
  pub fn from_polygon(poly: &[Coord]) -> (Self, ExtentType) {
    fn test_coordinates(poly: &[Coord]) -> Option<Extent> {
      if poly[0].y == poly[1].y {
        if poly[1].x == poly[2].x && poly[2].y == poly[3].y && poly[3].x == poly[0].x {
          return Some(Extent::new(poly[0].x..=poly[1].x, poly[2].y..=poly[1].y));
        }
      } else if poly[0].x == poly[1].x
        && poly[1].y == poly[2].y
        && poly[2].x == poly[3].x
        && poly[3].y == poly[0].y
      {
        return Some(Extent::new(poly[1].x..=poly[2].x, poly[0].y..=poly[1].y));
      }
      None
    }

    // Check if a polygon is an exact rectangle.
    if let Some(extent) = match poly.len() {
      4 => test_coordinates(poly),
      5 if poly[4] == poly[0] => test_coordinates(poly),
      _ => None,
    } {
      return (extent, ExtentType::Exact);
    }

    let mut min = Coord::new(f64::MAX, f64::MAX);
    let mut max = Coord::new(f64::MIN, f64::MIN);
    for coord in poly.iter() {
      min.x = min.x.min(coord.x);
      min.y = min.y.min(coord.y);
      max.x = max.x.max(coord.x);
      max.y = max.y.max(coord.y);
    }
    (
      Self::new(min.x..=max.x, min.y..=max.y),
      ExtentType::Contained,
    )
  }

  fn contains(&self, coord: Coord) -> bool {
    self.xr.contains(&coord.x) && self.yr.contains(&coord.y)
  }
}

pub struct Bounds {
  pub ext: Extent,
  pub poly: Vec<Coord>,
}

impl Bounds {
  pub fn new(poly: Vec<Coord>) -> Self {
    assert!(!poly.is_empty());

    let (ext, ext_type) = Extent::from_polygon(&poly);
    match ext_type {
      ExtentType::Contained => Self { ext, poly },
      ExtentType::Exact => {
        // A simple extent check will do.
        let poly = Vec::new();
        Self { ext, poly }
      }
    }
  }

  pub fn contains(&self, coord: Coord) -> bool {
    if self.ext.contains(coord) {
      if self.poly.is_empty() {
        return true;
      }
      return polygon_contains(&self.poly, coord);
    }
    false
  }
}

/// Check if a point is contained in a polygon.
fn polygon_contains(poly: &[Coord], point: Coord) -> bool {
  let mut inside = false;
  let count = poly.len();
  for idx in 0..count {
    let line = [poly[idx], poly[(idx + 1) % count]];

    // Check if the point is between the Y coordinates of the current line segment.
    if (line[0].y > point.y) != (line[1].y > point.y) {
      // Calculate the X coordinate where a horizontal ray from the point intersects the line segment.
      let x = (line[1].x - line[0].x) * (point.y - line[0].y) / (line[1].y - line[0].y) + line[0].x;

      // Check if the point lies to the left of the intersection.
      if point.x < x {
        // Toggle the inside flag.
        inside = !inside;
      }
    }
  }
  inside
}

pub trait Transform {
  fn transform(&self, coord: Coord) -> Result<Coord, gdal::errors::GdalError>;
}

impl Transform for spatial_ref::CoordTransform {
  fn transform(&self, coord: Coord) -> Result<Coord, gdal::errors::GdalError> {
    let mut x = [coord.x];
    let mut y = [coord.y];
    self.transform_coords(&mut x, &mut y, &mut [])?;
    Ok(Coord::new(x[0], y[0]))
  }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Pos {
  pub x: i32,
  pub y: i32,
}

impl Pos {
  pub fn from_variant(value: Variant) -> Option<Self> {
    use crate::util::ToI32;
    let value = value.try_to::<Array<Variant>>().ok()?;
    let x = value.get(0)?.to_i32()?;
    let y = value.get(1)?.to_i32()?;
    Some(Self { x, y })
  }

  pub fn to_variant(self) -> Variant {
    Variant::from([self.x, self.y])
  }
}

impl ops::Add for Pos {
  type Output = Self;

  fn add(mut self, offset: Pos) -> Self {
    self.x += offset.x;
    self.y += offset.y;
    self
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

impl From<Vector2i> for Pos {
  fn from(pos: Vector2i) -> Self {
    Self { x: pos.x, y: pos.y }
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

impl From<Pos> for Vector2i {
  fn from(pos: Pos) -> Self {
    Self { x: pos.x, y: pos.y }
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
  pub fn from_variant(value: Variant) -> Option<Self> {
    use crate::util::ToU32;
    let value = value.try_to::<Array<Variant>>().ok()?;
    let w = value.get(0)?.to_u32()?;
    let h = value.get(1)?.to_u32()?;
    Some(Self { w, h })
  }

  pub fn to_variant(self) -> Variant {
    Variant::from([self.w, self.h])
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

impl ops::Mul<f32> for Size {
  type Output = Self;

  fn mul(self, scale: f32) -> Self {
    Self {
      w: (self.w as f32 * scale).round() as u32,
      h: (self.h as f32 * scale).round() as u32,
    }
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

impl From<Vector2i> for Size {
  fn from(size: Vector2i) -> Self {
    Self {
      w: size.x as u32,
      h: size.y as u32,
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

impl From<Size> for Vector2i {
  fn from(size: Size) -> Self {
    Self {
      x: size.w as i32,
      y: size.h as i32,
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
  pub fn center(&self) -> Pos {
    let x = (self.pos.x + self.size.w as i32) / 2;
    let y = (self.pos.y + self.size.h as i32) / 2;
    Pos { x, y }
  }

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

mod test {
  #[test]
  fn polygon_contains() {
    use super::*;

    let points = [
      Coord::new(0.0, 0.0),
      Coord::new(100.0, 0.0),
      Coord::new(100.0, 100.0),
      Coord::new(0.0, 100.0),
      Coord::new(0.0, 75.0),
      Coord::new(50.0, 65.0),
      Coord::new(50.0, 15.0),
      Coord::new(0.0, 25.0),
    ];

    assert!(polygon_contains(&points, Coord::new(20.0, 10.0)));
    assert!(polygon_contains(&points, Coord::new(80.0, 80.0)));
    assert!(!polygon_contains(&points, Coord::new(20.0, 50.0)));
  }

  #[test]
  fn polygon_as_extent() {
    use super::*;

    let points = [
      Coord::new(0.0, 0.0),
      Coord::new(100.0, 0.0),
      Coord::new(100.0, 100.0),
      Coord::new(0.0, 100.0),
    ];

    assert!(Extent::from_polygon(&points).1 == ExtentType::Exact);

    let points = [
      Coord::new(0.0, 0.0),
      Coord::new(100.0, 0.0),
      Coord::new(100.0, 100.0),
      Coord::new(0.0, 100.0),
      Coord::new(0.0, 0.0),
    ];

    assert!(Extent::from_polygon(&points).1 == ExtentType::Exact);

    let points = [
      Coord::new(0.0, 0.0),
      Coord::new(100.0, 0.0),
      Coord::new(100.0, 100.0),
    ];

    assert!(Extent::from_polygon(&points).1 == ExtentType::Contained);

    let points = [
      Coord::new(0.0, 0.0),
      Coord::new(100.0, 1.0),
      Coord::new(100.0, 100.0),
      Coord::new(0.0, 100.0),
    ];

    assert!(Extent::from_polygon(&points).1 == ExtentType::Contained);

    let points = [
      Coord::new(0.0, 0.0),
      Coord::new(100.0, 0.0),
      Coord::new(100.0, 100.0),
      Coord::new(0.0, 100.0),
      Coord::new(0.0, 50.0),
    ];

    assert!(Extent::from_polygon(&points).1 == ExtentType::Contained);
  }
}
