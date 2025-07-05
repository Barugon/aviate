use crate::{
  nasr::{apt_base, common},
  util,
};
use gdal::{errors, vector};
use godot::global::godot_warn;
use std::{collections, path};

/// Dataset source for for `APT_RWY.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<Box<str>, Box<[u64]>>,
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
  pub fn create_index(&mut self, base_src: &apt_base::Source, cancel: util::Cancel) -> bool {
    use vector::LayerAccess;

    let base_id_map = base_src.id_map();
    let mut layer = self.layer();
    let mut id_map: collections::HashMap<String, Vec<u64>> = collections::HashMap::with_capacity(base_id_map.len());
    let mut add_fid = |id: String, fid: u64| {
      if let Some(id_vec) = id_map.get_mut(id.as_str()) {
        id_vec.push(fid);
      } else {
        id_map.insert(id, vec![fid]);
      }
    };

    // Iterator resets feature reading when dropped.
    for feature in layer.features() {
      if cancel.canceled() {
        return false;
      }

      if let Some(id) = common::get_string(&feature, self.fields.arpt_id)
        && base_id_map.contains_key(id.as_str())
        && let Some(fid) = feature.fid()
      {
        add_fid(id, fid);
      };
    }

    self.id_map = collections::HashMap::with_capacity(id_map.len());
    for (id, vec) in id_map {
      self.id_map.insert(id.into(), vec.into());
    }

    !self.id_map.is_empty()
  }

  pub fn clear_index(&mut self) {
    self.id_map = collections::HashMap::new();
  }

  /// Get runways for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn runways(&self, id: &str, cancel: util::Cancel) -> Vec<Runway> {
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

      if let Some(runway) = Runway::new(layer.feature(fid), &self.fields) {
        runways.push(runway);
        continue;
      }

      godot_warn!("Unable to read runway record #{fid}");
    }
    runways
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

/// Airport runway information.
#[derive(Clone, Debug)]
pub struct Runway {
  rwy_id: Box<str>,
  length: Box<str>,
  width: Box<str>,
  lighting: Box<str>,
  surface: Box<str>,
  condition: Box<str>,
}

impl Runway {
  fn new(feature: Option<vector::Feature>, fields: &Fields) -> Option<Self> {
    let feature = feature?;
    let rwy_id = common::get_string(&feature, fields.rwy_id)?.into();
    let length = get_length(&feature, fields)?.into();
    let width = get_width(&feature, fields)?.into();
    let lighting = get_lighting(&feature, fields)?.into();
    let surface = get_surface(&feature, fields)?.into();
    let condition = common::get_string(&feature, fields.cond)?.into();
    Some(Self {
      rwy_id,
      length,
      width,
      lighting,
      surface,
      condition,
    })
  }

  pub fn get_text(&self) -> String {
    self.get_id_text()
      + &self.get_length_text()
      + &self.get_width_text()
      + &self.get_lighting_text()
      + &self.get_surface_text()
      + &self.get_condition_text()
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

fn get_length(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  Some(format!("{} FEET", common::get_i64(feature, fields.rwy_len)?))
}

fn get_width(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  Some(format!("{} FEET", common::get_i64(feature, fields.rwy_width)?))
}

fn get_lighting(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  // Expand abbreviations.
  let lighting = common::get_string(feature, fields.rwy_lgt_code)?;
  Some(match lighting.as_str() {
    "MED" => "MEDIUM".into(),
    "NSTD" => "NON-STANDARD".into(),
    "PERI" => lighting, // Missing from layout doc.
    _ => lighting,
  })
}

fn get_surface(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let surface = common::get_string(feature, fields.surface_type_code)?;
  if surface.is_empty() {
    return Some(surface);
  }

  // Expand abbreviations.
  Some(match surface.as_str() {
    "ASPH" => "ASPHALT OR BITUMINOUS CONCRETE".into(),
    "ASPH-CONC" => surface, // Missing from layout doc.
    "CONC" => "PORTLAND CEMENT CONCRETE".into(),
    "DIRT" => "NATURAL SOIL".into(),
    "GRAVEL" => "GRAVEL; CINDERS; CRUSHED ROCK; CORAL OR SHELLS; SLAG".into(),
    "MATS" => "PIERCED STEEL PLANKING (PSP); LANDING MATS; MEMBRANES".into(),
    "PEM" => "PARTIALLY CONCRETE, ASPHALT OR BITUMEN-BOUND MACADAM".into(),
    "TREATED" => "OILED; SOIL CEMENT OR LIME STABILIZED".into(),
    "TURF" => "GRASS; SOD".into(),
    _ => surface,
  })
}
