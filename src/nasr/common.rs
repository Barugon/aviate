use crate::{geom, ok, util};
use gdal::{errors, spatial_ref, vector};
use godot::{classes::RegEx, obj::Gd};
use std::{borrow, cmp, collections, hash, slice};

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

pub type IDMap = collections::HashMap<util::StackString, u64>;

pub fn get_field_as_f64(feature: &vector::Feature, index: usize) -> Option<f64> {
  ok!(feature.field_as_double(index)).and_then(|v| v)
}

pub fn get_field_as_str<'a>(feature: &'a vector::Feature, index: usize) -> Option<&'a str> {
  if index >= feature.field_count() {
    return None;
  }

  let idx = ok!(index.try_into())?;
  if unsafe { gdal_sys::OGR_F_IsFieldNull(feature.c_feature(), idx) } != 0 {
    return None;
  }

  let mut len: i32 = 0;
  let ptr = unsafe { gdal_sys::OGR_F_GetFieldAsBinary(feature.c_feature(), idx, &mut len as *mut i32) };
  if ptr.is_null() || len < 0 {
    return None;
  }

  ok!(str::from_utf8(unsafe { slice::from_raw_parts(ptr, len as usize) }))
}

pub fn get_stack_string(feature: &vector::Feature, index: usize) -> Option<util::StackString> {
  util::StackString::from_str(get_field_as_str(feature, index)?)
}

pub fn get_yes_no_text<'a>(feature: &'a vector::Feature, index: usize) -> Option<&'a str> {
  let text = get_field_as_str(feature, index)?;
  Some(match text {
    "Y" => "YES",
    "N" => "NO",
    _ => text,
  })
}

pub fn get_unit_text(feature: &vector::Feature, unit: &str, index: usize) -> Option<String> {
  let text = get_field_as_str(feature, index)?;
  if text.is_empty() {
    return Some(String::new());
  }

  Some(format!("{text} {unit}"))
}

pub struct HashMapVec<K: cmp::Eq + hash::Hash, V> {
  map: collections::HashMap<K, Vec<V>>,
}

impl<K: cmp::Eq + hash::Hash, V> HashMapVec<K, V> {
  pub fn new(size: usize) -> Self {
    Self {
      map: collections::HashMap::with_capacity(size),
    }
  }

  pub fn push(&mut self, key: K, val: V) {
    if let Some(vec) = self.map.get_mut(&key) {
      vec.push(val);
    } else {
      self.map.insert(key, vec![val]);
    }
  }
}

impl<K: cmp::Eq + hash::Hash, V> From<HashMapVec<K, V>> for collections::HashMap<K, Box<[V]>> {
  fn from(src: HashMapVec<K, V>) -> Self {
    let mut dst = collections::HashMap::with_capacity(src.map.len());
    for (id, vec) in src.map {
      dst.insert(id, vec.into());
    }
    dst
  }
}

/// Search for and tag phone numbers in text.
pub struct PhoneTagger {
  regex: Option<Gd<RegEx>>,
}

impl PhoneTagger {
  pub fn new() -> Self {
    let regex = RegEx::create_from_string(r"\b(\d-)?\d{3}-\d{3}-\d{4}\b|\b1-800-WX-BRIEF\b");
    Self { regex }
  }

  pub fn process_text<'a>(&self, text: &'a str) -> borrow::Cow<'a, str> {
    let Some(regex) = &self.regex else {
      return text.into();
    };

    let mut ranges = Vec::new();
    for result in regex.search_all(text).iter_shared() {
      ranges.push((result.get_start() as usize, result.get_end() as usize));
    }

    if ranges.is_empty() {
      return text.into();
    }

    let mut tagged = String::new();
    let mut pos = 0;
    for (start, end) in ranges {
      let result = &text[start..end];
      let text = &text[pos..start];
      let text = if cfg!(target_os = "android") {
        format!("{text}[url=\"tel:{result}\"][color=#A0C0FF]{result}[/color][/url]")
      } else {
        format!("{text}[color=#A0C0FF]{result}[/color]")
      };
      tagged += &text;
      pos = end;
    }
    tagged += &text[pos..];
    tagged.into()
  }
}
