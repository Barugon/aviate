use crate::{
  nasr::{apt_base_csv, common},
  util,
};
use gdal::{errors, vector};
use std::{collections, path};

/// Dataset source for for `CLS_ARSP.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<util::StackString, u64>,
}

impl Source {
  /// Open a class airspace data source.
  /// - `path`: CSV zip file path
  pub fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let path = path::PathBuf::from(["/vsizip/", path.to_str().unwrap()].concat()).join("CLS_ARSP.csv");
    let dataset = gdal::Dataset::open_ex(path, common::open_options())?;
    let fields = Fields::new(dataset.layer(0)?)?;
    Ok(Self {
      dataset,
      fields,
      id_map: collections::HashMap::new(),
    })
  }

  /// Create the index.
  /// - `base_src`: airport base data source
  /// - `cancel`: cancellation object
  pub fn create_index(&mut self, base_src: &apt_base_csv::Source, cancel: &util::Cancel) -> bool {
    use vector::LayerAccess;

    let base_id_map = base_src.id_map();
    let mut layer = self.layer();
    let mut id_map = collections::HashMap::with_capacity(base_id_map.len());

    // Iterator resets feature reading when dropped.
    for feature in layer.features() {
      if cancel.canceled() {
        return false;
      }

      if let Some(id) = common::get_string(&feature, self.fields.arpt_id)
        && let Some(id) = util::StackString::from_str(&id)
        && base_id_map.contains_key(&id)
        && let Some(fid) = feature.fid()
      {
        id_map.insert(id, fid);
      };
    }

    self.id_map = id_map;
    !self.id_map.is_empty()
  }

  pub fn clear_index(&mut self) {
    self.id_map = collections::HashMap::new();
  }

  /// Get class airspace for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn class_airspace(&self, id: &str, cancel: &util::Cancel) -> Option<ClassAirspace> {
    use vector::LayerAccess;

    let &fid = self.id_map.get(&util::StackString::from_str(id)?)?;
    if cancel.canceled() {
      return None;
    }

    let layer = util::Layer::new(self.layer());
    ClassAirspace::new(layer.feature(fid), &self.fields)
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

/// Class airspace information.
pub struct ClassAirspace {
  airspace: Box<str>,
  hours: Box<str>,
  remark: Box<str>,
}

impl ClassAirspace {
  fn new(feature: Option<vector::Feature>, fields: &Fields) -> Option<Self> {
    let feature = feature?;
    Some(Self {
      airspace: get_class_airspace(&feature, fields)?.into(),
      hours: common::get_string(&feature, fields.airspace_hrs)?.into(),
      remark: common::get_string(&feature, fields.remark)?.into(),
    })
  }

  pub fn get_text(&self) -> String {
    self.get_class_airspace_text() + &self.get_hours_text() + &self.get_remark_text()
  }

  fn get_class_airspace_text(&self) -> String {
    format!("Airspace, Class: [color=white]{}[/color]\n", self.airspace)
  }

  fn get_hours_text(&self) -> String {
    if self.hours.is_empty() {
      return String::new();
    }
    format!("[ul] Hours: [color=white]{}[/color][/ul]\n", self.hours)
  }

  fn get_remark_text(&self) -> String {
    if self.remark.is_empty() {
      return String::new();
    }
    format!("[ul] [color=white]{}[/color][/ul]\n", self.remark)
  }
}

/// Field indexes for `CLS_ARSP.csv`.
struct Fields {
  arpt_id: usize,
  class_b_airspace: usize,
  class_c_airspace: usize,
  class_d_airspace: usize,
  class_e_airspace: usize,
  airspace_hrs: usize,
  remark: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;

    let defn = layer.defn();
    Ok(Self {
      arpt_id: defn.field_index("ARPT_ID")?,
      class_b_airspace: defn.field_index("CLASS_B_AIRSPACE")?,
      class_c_airspace: defn.field_index("CLASS_C_AIRSPACE")?,
      class_d_airspace: defn.field_index("CLASS_D_AIRSPACE")?,
      class_e_airspace: defn.field_index("CLASS_E_AIRSPACE")?,
      airspace_hrs: defn.field_index("AIRSPACE_HRS")?,
      remark: defn.field_index("REMARK")?,
    })
  }
}

fn get_class_airspace(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let mut airspaces = Vec::with_capacity(4);
  if let Some(b) = common::get_string(feature, fields.class_b_airspace)
    && b == "Y"
  {
    airspaces.push("B");
  }

  if let Some(c) = common::get_string(feature, fields.class_c_airspace)
    && c == "Y"
  {
    airspaces.push("C");
  }

  if let Some(d) = common::get_string(feature, fields.class_d_airspace)
    && d == "Y"
  {
    airspaces.push("D");
  }

  if let Some(e) = common::get_string(feature, fields.class_e_airspace)
    && e == "Y"
  {
    airspaces.push("E");
  }

  if airspaces.is_empty() {
    return None;
  }

  Some(airspaces.join(", "))
}
