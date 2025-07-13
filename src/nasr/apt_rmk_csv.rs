use crate::{
  nasr::{apt_base_csv, common},
  util,
};
use gdal::{errors, vector};
use std::{collections, path};

/// Dataset source for for `APT_RMK.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<util::StackString, Box<[u64]>>,
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
  pub fn create_index(&mut self, base_src: &apt_base_csv::Source, cancel: &util::Cancel) {
    use vector::LayerAccess;

    let base_id_map = base_src.id_map();
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

  /// Get remarks for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn remarks(&self, id: &str, cancel: &util::Cancel) -> Vec<Remark> {
    use vector::LayerAccess;

    let Some(fids) = util::StackString::from_str(id).and_then(|id| self.id_map.get(&id)) else {
      return Vec::new();
    };

    let layer = util::Layer::new(self.layer());
    let mut remarks = Vec::with_capacity(fids.len());
    for &fid in fids {
      if cancel.canceled() {
        return Vec::new();
      }

      if let Some(feature) = layer.feature(fid)
        && let Some(remark) = Remark::new(feature, &self.fields)
      {
        remarks.push(remark);
      }
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
pub struct Remark {
  reference: Box<str>,
  element: Box<str>,
  text: Box<str>,
}

impl Remark {
  fn new(feature: vector::Feature, fields: &Fields) -> Option<Self> {
    Some(Self {
      reference: get_reference(&feature, fields)?.into(),
      element: common::get_str(&feature, fields.element)?.into(),
      text: common::get_str(&feature, fields.remark)?.into(),
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
  let reference = match common::get_str(feature, fields.ref_col_name)? {
    "ARPT_ID" => "Airport ID",
    "ARPT_NAME" => "Airport Name",
    "BCN_LENS_COLOR" => "Beacon Color",
    "BCN_LGT_SKED" => "Beacon Schedule",
    "ELEV" => "Elevation",
    "FACILITY_USE_CODE" => "Facility Use",
    "FUEL_TYPE" => "Fuel Type",
    "GENERAL_REMARK" => Default::default(),
    "LGT_SKED" => "Lighting Schedule",
    "LNDG_FEE_FLAG" => "Landing Fee",
    "SITE_TYPE_CODE" => "Site Type",
    "TPA" => "Pattern Altitude",
    "SEG_CIRCLE_MKR_FLAG" => "Segmented Circle",
    _ => return None,
  };
  Some(reference.into())
}
