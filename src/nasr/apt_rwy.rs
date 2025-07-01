use crate::{
  nasr::{airport, apt_base, common},
  util,
};
use gdal::{errors, vector};
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
    use common::GetString;
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

      if let Some(id) = feature.get_string(self.fields.arpt_id)
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
  pub fn runways(&self, id: &str, cancel: util::Cancel) -> Option<Vec<airport::Runway>> {
    use vector::LayerAccess;

    let fids = self.id_map.get(id)?;
    let layer = util::Layer::new(self.layer());
    let mut runways = Vec::with_capacity(fids.len());
    for &fid in fids {
      if cancel.canceled() {
        return None;
      }
      runways.push(airport::Runway::new(layer.feature(fid), &self.fields)?);
    }
    Some(runways)
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

impl airport::Runway {
  fn new(feature: Option<vector::Feature>, fields: &Fields) -> Option<Self> {
    use common::GetString;

    let feature = feature?;
    let rwy_id = feature.get_string(fields.rwy_id)?.into();
    let length = feature.get_length(fields)?.into();
    let width = feature.get_width(fields)?.into();
    let lighting = feature.get_lighting(fields)?.into();
    let surface = feature.get_surface(fields)?.into();
    let condition = feature.get_string(fields.cond)?.into();
    Some(Self {
      rwy_id,
      length,
      width,
      lighting,
      surface,
      condition,
    })
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

trait GetLength {
  fn get_length(&self, fields: &Fields) -> Option<String>;
}

impl GetLength for vector::Feature<'_> {
  fn get_length(&self, fields: &Fields) -> Option<String> {
    use common::GetI64;

    Some(format!("{} FEET", self.get_i64(fields.rwy_len)?))
  }
}

trait GetWidth {
  fn get_width(&self, fields: &Fields) -> Option<String>;
}

impl GetWidth for vector::Feature<'_> {
  fn get_width(&self, fields: &Fields) -> Option<String> {
    use common::GetI64;

    Some(format!("{} FEET", self.get_i64(fields.rwy_width)?))
  }
}

trait GetLighting {
  fn get_lighting(&self, fields: &Fields) -> Option<String>;
}

impl GetLighting for vector::Feature<'_> {
  fn get_lighting(&self, fields: &Fields) -> Option<String> {
    use common::GetString;

    // Expand abbreviations.
    let lighting = self.get_string(fields.rwy_lgt_code)?;
    Some(match lighting.as_str() {
      "MED" => String::from("MEDIUM"),
      "NSTD" => String::from("NON-STANDARD"),
      "PERI" => lighting, // Missing from layout doc.
      _ => lighting,
    })
  }
}

trait GetSurface {
  fn get_surface(&self, fields: &Fields) -> Option<String>;
}

impl GetSurface for vector::Feature<'_> {
  fn get_surface(&self, fields: &Fields) -> Option<String> {
    use common::GetString;

    let surface = self.get_string(fields.surface_type_code)?;
    if surface.is_empty() {
      return Some(surface);
    }

    // Expand abbreviations.
    Some(match surface.as_str() {
      "ASPH" => String::from("ASPHALT OR BITUMINOUS CONCRETE"),
      "ASPH-CONC" => surface, // Missing from layout doc.
      "CONC" => String::from("PORTLAND CEMENT CONCRETE"),
      "DIRT" => String::from("NATURAL SOIL"),
      "GRAVEL" => String::from("GRAVEL; CINDERS; CRUSHED ROCK; CORAL OR SHELLS; SLAG"),
      "MATS" => String::from("PIERCED STEEL PLANKING (PSP); LANDING MATS; MEMBRANES"),
      "PEM" => String::from("PARTIALLY CONCRETE, ASPHALT OR BITUMEN-BOUND MACADAM"),
      "TREATED" => String::from("OILED; SOIL CEMENT OR LIME STABILIZED"),
      "TURF" => String::from("GRASS; SOD"),
      _ => surface,
    })
  }
}
