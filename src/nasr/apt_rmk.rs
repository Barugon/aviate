use crate::{
  nasr::{apt_base, common},
  util,
};
use gdal::{errors, vector};
use godot::global::godot_warn;
use std::{collections, path};

/// Dataset source for for `APT_RMK.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<Box<str>, Box<[u64]>>,
}

impl Source {
  /// Open an airport remark data source.
  /// - `path`: CSV zip file path
  pub fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let path = path::PathBuf::from(["/vsizip/", path.to_str().unwrap()].concat()).join("APT_RMK.csv");
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

  /// Get remarks for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn remarks(&self, id: &str, cancel: util::Cancel) -> Vec<Remark> {
    use vector::LayerAccess;

    let Some(fids) = self.id_map.get(id) else {
      return Vec::new();
    };

    let layer = util::Layer::new(self.layer());
    let mut remarks = Vec::with_capacity(fids.len());
    for &fid in fids {
      if cancel.canceled() {
        return Vec::new();
      }

      if let Some(remark) = Remark::new(layer.feature(fid), &self.fields) {
        remarks.push(remark);
        continue;
      }

      godot_warn!("Unable to read remark record #{fid}");
    }
    remarks
  }

  pub fn clear_index(&mut self) {
    self.id_map = collections::HashMap::new();
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

/// Airport remark information.
#[derive(Clone, Debug)]
pub struct Remark {
  reference: Box<str>,
  element: Box<str>,
  text: Box<str>,
}

impl Remark {
  fn new(feature: Option<vector::Feature>, fields: &Fields) -> Option<Self> {
    let feature = feature?;
    let reference = get_reference(&feature, fields)?.into();
    let element = common::get_string(&feature, fields.element)?.into();
    let text = common::get_string(&feature, fields.remark)?.into();

    Some(Self {
      reference,
      element,
      text,
    })
  }

  pub fn get_text(&self, phone_tagger: &common::PhoneTagger) -> String {
    let text = phone_tagger.process_text(&self.text);
    let element = &self.element;
    let reference = &self.reference;
    if reference.is_empty() {
      // General remark.
      format!("[ul] [color=white]{text}[/color][/ul]\n")
    } else if element.is_empty() {
      format!("[ul] {reference}: [color=white]{text}[/color][/ul]\n")
    } else {
      format!("[ul] {reference} ({element}): [color=white]{text}[/color][/ul]\n")
    }
  }
}

/// Field indexes for `APT_RMK.csv`.
struct Fields {
  arpt_id: usize,
  element: usize,
  ref_col_name: usize,
  remark: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;

    let defn = layer.defn();
    Ok(Self {
      arpt_id: defn.field_index("ARPT_ID")?,
      element: defn.field_index("ELEMENT")?,
      ref_col_name: defn.field_index("REF_COL_NAME")?,
      remark: defn.field_index("REMARK")?,
    })
  }
}

fn get_reference(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  Some(match common::get_string(feature, fields.ref_col_name)?.as_str() {
    "ARPT_ID" => String::from("Airport ID"),
    "ARPT_NAME" => String::from("Airport Name"),
    "BCN_LENS_COLOR" => String::from("Beacon Color"),
    "BCN_LGT_SKED" => String::from("Beacon Schedule"),
    "ELEV" => String::from("Elevation"),
    "FACILITY_USE_CODE" => String::from("Facility Use"),
    "FUEL_TYPE" => String::from("Fuel Type"),
    "GENERAL_REMARK" => String::new(),
    "LGT_SKED" => String::from("Lighting Schedule"),
    "LNDG_FEE_FLAG" => String::from("Landing Fee"),
    "SITE_TYPE_CODE" => String::from("Site Type"),
    "TPA" => String::from("Pattern Altitude"),
    _ => return None,
  })
}
