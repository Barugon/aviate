use crate::geom;
use gdal::{errors, spatial_ref, vector};
use godot::global::godot_error;

pub fn open_options<'a>() -> gdal::DatasetOptions<'a> {
  gdal::DatasetOptions {
    open_flags: gdal::GdalOpenFlags::GDAL_OF_READONLY
      | gdal::GdalOpenFlags::GDAL_OF_VECTOR
      | gdal::GdalOpenFlags::GDAL_OF_INTERNAL,
    ..Default::default()
  }
}

pub struct ToChart {
  /// Coordinate transformation from decimal-degree coordinates to chart coordinates.
  trans: spatial_ref::CoordTransform,

  /// Chart bounds.
  bounds: geom::Bounds,
}

impl ToChart {
  pub fn new(proj4: &str, dd_sr: &spatial_ref::SpatialRef, bounds: geom::Bounds) -> errors::Result<Self> {
    // Create a transformation from decimal-degree coordinates to chart coordinates.
    let chart_sr = spatial_ref::SpatialRef::from_proj4(proj4)?;
    let trans = spatial_ref::CoordTransform::new(dd_sr, &chart_sr)?;
    Ok(ToChart { trans, bounds })
  }

  pub fn transform(&self, coord: geom::DD) -> errors::Result<geom::Cht> {
    use geom::Transform;
    Ok(self.trans.transform(*coord)?.into())
  }

  pub fn bounds(&self) -> &geom::Bounds {
    &self.bounds
  }
}

pub trait GetI64 {
  fn get_i64(&self, index: usize) -> Option<i64>;
}

impl GetI64 for vector::Feature<'_> {
  fn get_i64(&self, index: usize) -> Option<i64> {
    match self.field_as_integer64(index) {
      Ok(val) => val,
      Err(err) => {
        godot_error!("{err}");
        None
      }
    }
  }
}

pub trait GetF64 {
  fn get_f64(&self, index: usize) -> Option<f64>;
}

impl GetF64 for vector::Feature<'_> {
  fn get_f64(&self, index: usize) -> Option<f64> {
    match self.field_as_double(index) {
      Ok(val) => val,
      Err(err) => {
        godot_error!("{err}");
        None
      }
    }
  }
}

pub trait GetString {
  fn get_string(&self, index: usize) -> Option<String>;
}

impl GetString for vector::Feature<'_> {
  fn get_string(&self, index: usize) -> Option<String> {
    match self.field_as_string(index) {
      Ok(val) => val,
      Err(err) => {
        godot_error!("{err}");
        None
      }
    }
  }
}
