use crate::{
  nasr::{apt_rwy_end_csv, common},
  util,
};
use gdal::{errors, vector};
use godot::global::godot_warn;
use std::{collections, path};

/// Dataset source for for `APT_RWY.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<util::StackString, Box<[u64]>>,
}

impl Source {
  /// Open an airport runway data source.
  /// - `path`: CSV zip file path
  pub fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let path = path::PathBuf::from(["/vsizip/", path.to_str().unwrap()].concat()).join("APT_RWY.csv");
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
  pub fn create_index(&mut self, base_id_map: &common::IDMap, cancel: &util::Cancel) {
    use vector::LayerAccess;

    let mut layer = self.layer();
    let mut id_map = common::HashMapVec::new(base_id_map.len());

    // Iterator resets feature reading when dropped.
    for feature in layer.features() {
      if cancel.canceled() {
        return;
      }

      if let Some(id) = common::get_stack_string(&feature, self.fields.arpt_id)
        && base_id_map.contains_key(&id)
        && let Some(fid) = feature.fid()
      {
        id_map.push(id, fid);
      };
    }

    self.id_map = id_map.into();
  }

  pub fn clear_index(&mut self) {
    self.id_map = collections::HashMap::new();
  }

  /// Get runways for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn runways(
    &self,
    id: &util::StackString,
    mut ends_map: apt_rwy_end_csv::RunwayEndMap,
    cancel: &util::Cancel,
  ) -> Vec<Runway> {
    use vector::LayerAccess;

    let Some(fids) = self.id_map.get(id) else {
      return Vec::new();
    };

    let layer = util::Layer::new(self.layer());
    let mut runways = Vec::with_capacity(fids.len());
    for &fid in fids {
      if cancel.canceled() {
        return Vec::new();
      }

      if let Some(feature) = layer.feature(fid)
        && let Some(runway) = Runway::new(feature, &mut ends_map, &self.fields)
      {
        runways.push(runway);
        continue;
      }

      godot_warn!("Unable to read runway record #{fid}");
    }
    runways
  }

  fn layer(&self) -> vector::Layer<'_> {
    self.dataset.layer(0).unwrap()
  }
}

/// Airport runway information.
pub struct Runway {
  rwy_id: Box<str>,
  length: Box<str>,
  width: Box<str>,
  lighting: Box<str>,
  surface: Box<str>,
  condition: Box<str>,
  ends: Box<[apt_rwy_end_csv::RunwayEnd]>,
}

impl Runway {
  fn new(feature: vector::Feature, ends_map: &mut apt_rwy_end_csv::RunwayEndMap, fields: &Fields) -> Option<Self> {
    const FEET: &str = "FEET";
    let rwy_id = common::get_field_as_str(&feature, fields.rwy_id)?;
    let ends = ends_map.remove(rwy_id).map(|ends| ends.into()).unwrap_or_default();
    Some(Self {
      rwy_id: rwy_id.into(),
      length: common::get_unit_text(&feature, FEET, fields.rwy_len)?.into(),
      width: common::get_unit_text(&feature, FEET, fields.rwy_width)?.into(),
      lighting: get_lighting(&feature, fields)?.into(),
      surface: get_surface(&feature, fields)?.into(),
      condition: common::get_field_as_str(&feature, fields.cond)?.into(),
      ends,
    })
  }

  pub fn get_text(&self) -> String {
    let mut text = self.get_id_text()
      + &self.get_length_text()
      + &self.get_width_text()
      + &self.get_lighting_text()
      + &self.get_surface_text()
      + &self.get_condition_text();

    for end in &self.ends {
      text += "[indent]";
      text += &end.get_text();
      text += "[/indent]";
    }

    text
  }

  fn get_id_text(&self) -> String {
    format!("\nRunway: [color=#FFD090]{}[/color]\n", self.rwy_id)
  }

  fn get_length_text(&self) -> String {
    format!("[ul] Length: [color=white]{}[/color][/ul]\n", self.length)
  }

  fn get_width_text(&self) -> String {
    format!("[ul] Width: [color=white]{}[/color][/ul]\n", self.width)
  }

  fn get_lighting_text(&self) -> String {
    if self.lighting.is_empty() {
      return String::new();
    }
    format!("[ul] Lighting: [color=white]{}[/color][/ul]\n", self.lighting)
  }

  fn get_surface_text(&self) -> String {
    if self.surface.is_empty() {
      return String::new();
    }
    format!("[ul] Surface: [color=white]{}[/color][/ul]\n", self.surface)
  }

  fn get_condition_text(&self) -> String {
    if self.condition.is_empty() {
      return String::new();
    }
    format!("[ul] Condition: [color=white]{}[/color][/ul]\n", self.condition)
  }
}

/// Field indexes for `APT_BASE.csv`.
struct Fields {
  arpt_id: usize,
  rwy_id: usize,
  rwy_len: usize,
  rwy_width: usize,
  rwy_lgt_code: usize,
  surface_type_code: usize,
  cond: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;

    let defn = layer.defn();
    Ok(Self {
      arpt_id: defn.field_index("ARPT_ID")?,
      rwy_id: defn.field_index("RWY_ID")?,
      rwy_len: defn.field_index("RWY_LEN")?,
      rwy_width: defn.field_index("RWY_WIDTH")?,
      rwy_lgt_code: defn.field_index("RWY_LGT_CODE")?,
      surface_type_code: defn.field_index("SURFACE_TYPE_CODE")?,
      cond: defn.field_index("COND")?,
    })
  }
}

fn get_lighting<'a>(feature: &'a vector::Feature, fields: &Fields) -> Option<&'a str> {
  let lighting = common::get_field_as_str(feature, fields.rwy_lgt_code)?;
  Some(match lighting {
    "MED" => "MEDIUM",
    "NSTD" => "NON-STANDARD",
    "PERI" => lighting, // Missing from layout doc.
    _ => lighting,
  })
}

fn get_surface<'a>(feature: &'a vector::Feature, fields: &Fields) -> Option<&'a str> {
  let surface = common::get_field_as_str(feature, fields.surface_type_code)?;
  Some(match surface {
    "ASPH" => "ASPHALT OR BITUMINOUS CONCRETE",
    "ASPH-CONC" => surface, // Missing from layout doc.
    "CONC" => "PORTLAND CEMENT CONCRETE",
    "DIRT" => "NATURAL SOIL",
    "GRAVEL" => "GRAVEL; CINDERS; CRUSHED ROCK; CORAL OR SHELLS; SLAG",
    "MATS" => "PIERCED STEEL PLANKING (PSP); LANDING MATS; MEMBRANES",
    "PEM" => "PARTIALLY CONCRETE, ASPHALT OR BITUMEN-BOUND MACADAM",
    "TREATED" => "OILED; SOIL CEMENT OR LIME STABILIZED",
    "TURF" => "GRASS; SOD",
    _ => surface,
  })
}
