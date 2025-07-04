use crate::{geom, ok};
use gdal::{errors, spatial_ref, vector};
use godot::classes::RegEx;
use std::borrow;

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
    self.trans.transform(*coord).map(|c| c.into())
  }

  pub fn bounds(&self) -> &geom::Bounds {
    &self.bounds
  }
}

pub fn get_i64(feature: &vector::Feature, index: usize) -> Option<i64> {
  ok!(feature.field_as_integer64(index)).and_then(|v| v)
}

pub fn get_f64(feature: &vector::Feature, index: usize) -> Option<f64> {
  ok!(feature.field_as_double(index)).and_then(|v| v)
}

pub fn get_string(feature: &vector::Feature, index: usize) -> Option<String> {
  ok!(feature.field_as_string(index)).and_then(|v| v)
}

pub fn tag_phone_numbers<'a>(text: &'a str) -> borrow::Cow<'a, str> {
  // TODO: Enable only for phones.
  let mut ranges = Vec::new();
  if let Some(regex) = RegEx::create_from_string(r"\b\d{3}-\d{3}-\d{4}\b") {
    for result in regex.search_all(text).iter_shared() {
      ranges.push((result.get_start() as usize, result.get_end() as usize));
    }
  }

  if !ranges.is_empty() {
    let mut tagged = String::new();
    let mut pos = 0;
    for (start, end) in ranges {
      tagged += &format!(
        "{}[url][color=#A0C0FF]{}[/color][/url]",
        &text[pos..start],
        &text[start..end],
      );
      pos = end;
    }
    tagged += &text[pos..];
    return tagged.into();
  }

  text.into()
}
