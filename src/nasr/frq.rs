use crate::{
  nasr::{apt_base, common},
  util,
};
use gdal::{errors, vector};
use std::{collections, path};

/// Dataset source for for `FRQ.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<Box<str>, Box<[u64]>>,
}

impl Source {
  /// Open a frequency data source.
  /// - `path`: CSV zip file path
  pub fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let path = path::PathBuf::from(["/vsizip/", path.to_str().unwrap()].concat()).join("FRQ.csv");
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

      if let Some(id) = common::get_string(&feature, self.fields.serviced_facility)
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

  /// Get frequencies for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn frequencies(&self, id: &str, cancel: util::Cancel) -> Option<Vec<Frequency>> {
    use vector::LayerAccess;

    let fids = self.id_map.get(id)?;
    let layer = util::Layer::new(self.layer());
    let mut frequencies = Vec::with_capacity(fids.len());
    for &fid in fids {
      if cancel.canceled() {
        return None;
      }
      frequencies.push(Frequency::new(layer.feature(fid), &self.fields)?);
    }
    Some(frequencies)
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

/// Airport frequency information.
#[derive(Clone, Debug)]
pub struct Frequency {
  freq: Box<str>,
  freq_use: Box<str>,
  facility_type: Box<str>,
  sectorization: Box<str>,
  tower_call: Box<str>,
  approach_call: Box<str>,
  remark: Box<str>,
}

impl Frequency {
  fn new(feature: Option<vector::Feature>, fields: &Fields) -> Option<Self> {
    let feature = feature?;
    let freq = common::get_string(&feature, fields.freq)?.into();
    let freq_use = common::get_string(&feature, fields.freq_use)?.into();
    let facility_type = common::get_string(&feature, fields.facility_type)?.into();
    let sectorization = common::get_string(&feature, fields.sectorization)?.into();
    let tower_call = common::get_string(&feature, fields.tower_or_comm_call)?.into();
    let approach_call = common::get_string(&feature, fields.primary_approach_radio_call)?.into();
    let remark = common::get_string(&feature, fields.remark)?.into();
    Some(Self {
      freq,
      freq_use,
      facility_type,
      sectorization,
      tower_call,
      approach_call,
      remark,
    })
  }

  pub fn get_text(&self, phone_tagger: &common::PhoneTagger) -> String {
    self.get_frequency_text()
      + &self.get_frequency_use_text()
      + &self.get_facility_type_text()
      + &self.get_sectorization_text()
      + &self.get_tower_call_text()
      + &self.get_approach_call_text()
      + &self.get_remark_text(phone_tagger)
  }

  fn get_frequency_text(&self) -> String {
    format!("\nFrequency: [color=#A0FFA0]{}[/color]\n", self.freq)
  }

  fn get_frequency_use_text(&self) -> String {
    if self.freq_use.is_empty() {
      return String::new();
    }
    format!("[ul] Use: [color=white]{}[/color][/ul]\n", self.freq_use)
  }

  fn get_facility_type_text(&self) -> String {
    if self.facility_type.is_empty() {
      return String::new();
    }
    format!("[ul] Facility Type: [color=white]{}[/color][/ul]\n", self.facility_type)
  }

  fn get_sectorization_text(&self) -> String {
    if self.sectorization.is_empty() {
      return String::new();
    }
    format!("[ul] Sectorization: [color=white]{}[/color][/ul]\n", self.sectorization)
  }

  fn get_tower_call_text(&self) -> String {
    if self.tower_call.is_empty() {
      return String::new();
    }
    format!("[ul] Tower/Comm Call: [color=white]{}[/color][/ul]\n", self.tower_call)
  }

  fn get_approach_call_text(&self) -> String {
    if self.approach_call.is_empty() {
      return String::new();
    }
    format!("[ul] Approach Call: [color=white]{}[/color][/ul]\n", self.approach_call)
  }

  fn get_remark_text(&self, phone_tagger: &common::PhoneTagger) -> String {
    if self.remark.is_empty() {
      return String::new();
    }
    let text = phone_tagger.process_text(&self.remark);
    format!("[ul] [color=white]{text}[/color][/ul]\n")
  }
}

/// Field indexes for `FRQ.csv`.
struct Fields {
  facility_type: usize,
  freq_use: usize,
  freq: usize,
  primary_approach_radio_call: usize,
  remark: usize,
  sectorization: usize,
  serviced_facility: usize,
  tower_or_comm_call: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;

    let defn = layer.defn();
    Ok(Self {
      facility_type: defn.field_index("FACILITY_TYPE")?,
      freq_use: defn.field_index("FREQ_USE")?,
      freq: defn.field_index("FREQ")?,
      primary_approach_radio_call: defn.field_index("PRIMARY_APPROACH_RADIO_CALL")?,
      remark: defn.field_index("REMARK")?,
      sectorization: defn.field_index("SECTORIZATION")?,
      serviced_facility: defn.field_index("SERVICED_FACILITY")?,
      tower_or_comm_call: defn.field_index("TOWER_OR_COMM_CALL")?,
    })
  }
}
