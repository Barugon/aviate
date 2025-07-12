use crate::{
  nasr::{apt_base_csv, common},
  util,
};
use gdal::{errors, vector};
use godot::global::godot_warn;
use std::{collections, path};

/// Dataset source for for `APT_RWY_END.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<util::StackString, Box<[u64]>>,
}

impl Source {
  /// Open an airport runway data source.
  /// - `path`: CSV zip file path
  pub fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let path = path::PathBuf::from(["/vsizip/", path.to_str().unwrap()].concat()).join("APT_RWY_END.csv");
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
    let mut id_map = common::HashMapVec::new(base_id_map.len());

    // Iterator resets feature reading when dropped.
    for feature in layer.features() {
      if cancel.canceled() {
        return false;
      }

      if let Some(id) = common::get_stack_string(&feature, self.fields.arpt_id)
        && base_id_map.contains_key(&id)
        && let Some(fid) = feature.fid()
      {
        id_map.push(id, fid);
      };
    }

    self.id_map = id_map.into();
    !self.id_map.is_empty()
  }

  pub fn clear_index(&mut self) {
    self.id_map = collections::HashMap::new();
  }

  /// Get runway ends for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn runway_ends(&self, id: &str, cancel: &util::Cancel) -> RunwayEndMap {
    use vector::LayerAccess;

    let Some(fids) = util::StackString::from_str(id).and_then(|id| self.id_map.get(&id)) else {
      return collections::HashMap::new();
    };

    let layer = util::Layer::new(self.layer());
    let mut runway_ends = RunwayEndMap::with_capacity(fids.len());
    let mut add_rwy_end = |id: String, rwy_end: RunwayEnd| {
      if let Some(rwy_id_vec) = runway_ends.get_mut(id.as_str()) {
        rwy_id_vec.push(rwy_end);
      } else {
        runway_ends.insert(id, vec![rwy_end]);
      }
    };

    for &fid in fids {
      if cancel.canceled() {
        return collections::HashMap::new();
      }

      if let Some(feature) = layer.feature(fid)
        && let Some(rwy_id) = common::get_string(&feature, self.fields.rwy_id)
        && let Some(runway) = RunwayEnd::new(feature, &self.fields)
      {
        add_rwy_end(rwy_id, runway);
        continue;
      }

      godot_warn!("Unable to read runway end record #{fid}");
    }
    runway_ends
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

pub type RunwayEndMap = collections::HashMap<String, Vec<RunwayEnd>>;

/// Airport runway information.
pub struct RunwayEnd {
  rwy_end_id: Box<str>,
  elevation: Box<str>,
  true_alignment: Box<str>,
  rh_traffic: Box<str>,
  markings: Box<str>,
  gld_slp_ind: Box<str>,
  displaced_thr_elev: Box<str>,
  displaced_thr_len: Box<str>,
  tdz_elevation: Box<str>,
  obstacle: Box<str>,
  obstacle_height: Box<str>,
  obstacle_offset: Box<str>,
}

impl RunwayEnd {
  fn new(feature: vector::Feature, fields: &Fields) -> Option<Self> {
    Some(Self {
      rwy_end_id: common::get_string(&feature, fields.rwy_end_id)?.into(),
      elevation: common::get_unit_text(&feature, "FEET ASL", fields.rwy_end_elev)?.into(),
      true_alignment: get_true_alignment(&feature, fields)?.into(),
      rh_traffic: common::get_yes_no_text(&feature, fields.right_hand_traffic_pat_flag)?.into(),
      markings: get_markings(&feature, fields)?.into(),
      gld_slp_ind: get_glide_slope_indicator(&feature, fields)?.into(),
      displaced_thr_elev: common::get_unit_text(&feature, "FEET ASL", fields.displaced_thr_elev)?.into(),
      displaced_thr_len: common::get_unit_text(&feature, "FEET", fields.displaced_thr_len)?.into(),
      tdz_elevation: common::get_unit_text(&feature, "FEET ASL", fields.tdz_elev)?.into(),
      obstacle: get_obstacle(&feature, fields)?.into(),
      obstacle_height: common::get_unit_text(&feature, "FEET", fields.obstn_hgt)?.into(),
      obstacle_offset: get_obstacle_offset(&feature, fields)?.into(),
    })
  }

  pub fn get_text(&self) -> String {
    if self.elevation.is_empty()
      && self.true_alignment.is_empty()
      && self.rh_traffic.is_empty()
      && self.markings.is_empty()
      && self.gld_slp_ind.is_empty()
      && self.displaced_thr_elev.is_empty()
      && self.displaced_thr_len.is_empty()
      && self.tdz_elevation.is_empty()
      && self.obstacle.is_empty()
      && self.obstacle_height.is_empty()
      && self.obstacle_offset.is_empty()
    {
      return String::new();
    }

    self.get_id_text()
      + &self.get_elevation_text()
      + &self.get_true_alignment_text()
      + &self.get_rh_traffic_text()
      + &self.get_markings_text()
      + &self.get_gld_slp_ind_text()
      + &self.get_displaced_threshold_elevation_text()
      + &self.get_displaced_threshold_length_text()
      + &self.get_touchdown_zone_elevation_text()
      + &self.get_obstacle_text()
      + &self.get_obstacle_height_text()
      + &self.get_obstacle_offset_text()
  }

  fn get_id_text(&self) -> String {
    format!("End: [color=white]{}[/color]\n", self.rwy_end_id)
  }

  fn get_elevation_text(&self) -> String {
    let text = &self.elevation;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Elevation: [color=white]{text}[/color][/ul]\n")
  }

  fn get_true_alignment_text(&self) -> String {
    let text = &self.true_alignment;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] True Alignment: [color=white]{text}[/color][/ul]\n")
  }

  fn get_rh_traffic_text(&self) -> String {
    let text = &self.rh_traffic;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Right Hand Traffic: [color=white]{text}[/color][/ul]\n",)
  }

  fn get_markings_text(&self) -> String {
    let text = &self.markings;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Markings: [color=white]{text}[/color][/ul]\n")
  }

  fn get_gld_slp_ind_text(&self) -> String {
    let text = &self.gld_slp_ind;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Glide Slope Indicator: [color=white]{text}[/color][/ul]\n")
  }

  fn get_displaced_threshold_elevation_text(&self) -> String {
    let text = &self.displaced_thr_elev;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Displaced Threshold Elevation: [color=white]{text}[/color][/ul]\n")
  }

  fn get_displaced_threshold_length_text(&self) -> String {
    let text = &self.displaced_thr_len;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Displaced Threshold Length: [color=white]{text}[/color][/ul]\n")
  }

  fn get_touchdown_zone_elevation_text(&self) -> String {
    let text = &self.tdz_elevation;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Touchdown Zone Elevation: [color=white]{text}[/color][/ul]\n")
  }

  fn get_obstacle_text(&self) -> String {
    let text = &self.obstacle;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Obstacle: [color=white]{text}[/color][/ul]\n")
  }

  fn get_obstacle_height_text(&self) -> String {
    let text = &self.obstacle_height;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Obstacle Height: [color=white]{text}[/color][/ul]\n")
  }

  fn get_obstacle_offset_text(&self) -> String {
    let text = &self.obstacle_offset;
    if text.is_empty() {
      return String::new();
    }
    format!("[ul] Distance From Threshold: [color=white]{text}[/color][/ul]\n")
  }
}

/// Field indexes for `APT_BASE.csv`.
struct Fields {
  arpt_id: usize,
  cntrln_dir_code: usize,
  cntrln_offset: usize,
  displaced_thr_elev: usize,
  displaced_thr_len: usize,
  dist_from_thr: usize,
  obstn_hgt: usize,
  obstn_mrkd_code: usize,
  obstn_type: usize,
  right_hand_traffic_pat_flag: usize,
  rwy_end_elev: usize,
  rwy_end_id: usize,
  rwy_id: usize,
  rwy_marking_cond: usize,
  rwy_marking_type_code: usize,
  tdz_elev: usize,
  true_alignment: usize,
  vgsi_code: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;

    let defn = layer.defn();
    Ok(Self {
      arpt_id: defn.field_index("ARPT_ID")?,
      cntrln_dir_code: defn.field_index("CNTRLN_DIR_CODE")?,
      cntrln_offset: defn.field_index("CNTRLN_OFFSET")?,
      displaced_thr_elev: defn.field_index("DISPLACED_THR_ELEV")?,
      displaced_thr_len: defn.field_index("DISPLACED_THR_LEN")?,
      dist_from_thr: defn.field_index("DIST_FROM_THR")?,
      obstn_hgt: defn.field_index("OBSTN_HGT")?,
      obstn_mrkd_code: defn.field_index("OBSTN_MRKD_CODE")?,
      obstn_type: defn.field_index("OBSTN_TYPE")?,
      right_hand_traffic_pat_flag: defn.field_index("RIGHT_HAND_TRAFFIC_PAT_FLAG")?,
      rwy_end_elev: defn.field_index("RWY_END_ELEV")?,
      rwy_end_id: defn.field_index("RWY_END_ID")?,
      rwy_id: defn.field_index("RWY_ID")?,
      rwy_marking_cond: defn.field_index("RWY_MARKING_COND")?,
      rwy_marking_type_code: defn.field_index("RWY_MARKING_TYPE_CODE")?,
      tdz_elev: defn.field_index("TDZ_ELEV")?,
      true_alignment: defn.field_index("TRUE_ALIGNMENT")?,
      vgsi_code: defn.field_index("VGSI_CODE")?,
    })
  }
}

fn get_true_alignment(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let mut alignment = common::get_string(feature, fields.true_alignment)?;
  if !alignment.is_empty() {
    alignment += "°";
  }
  Some(alignment)
}

fn get_markings(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let markings = common::get_string(feature, fields.rwy_marking_type_code)?;
  let markings = match markings.as_str() {
    "PIR" => "PRECISION INSTRUMENT".into(),
    "NPI" => "NONPRECISION INSTRUMENT".into(),
    "BSC" => "BASIC".into(),
    "NRS" => "NUMBERS ONLY".into(),
    "NSTD" => "NONSTANDARD (OTHER THAN NUMBERS ONLY)".into(),
    "BUOY" => "BUOYS (SEAPLANE BASE)".into(),
    "STOL" => "SHORT TAKEOFF AND LANDING".into(),
    _ => markings,
  };

  if !markings.is_empty() {
    let condition = common::get_string(feature, fields.rwy_marking_cond)?;
    let condition = match condition.as_str() {
      "G" => "GOOD".into(),
      "F" => "FAIR".into(),
      "P" => "POOR".into(),
      _ => condition,
    };

    if !condition.is_empty() {
      return Some(format!("{markings}, {condition} CONDITION"));
    }
  }

  Some(markings)
}

fn get_glide_slope_indicator(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let vgsi = common::get_string(feature, fields.vgsi_code)?;
  Some(match vgsi.as_str() {
    "N" => String::new(),
    "NSTD" => "NONSTANDARD VASI SYSTEM".into(),
    "P2L" => "2-LGT PAPI ON LEFT SIDE OF RUNWAY".into(),
    "P2R" => "2-LGT PAPI ON RIGHT SIDE OF RUNWAY".into(),
    "P4L" => "4-LGT PAPI ON LEFT SIDE OF RUNWAY".into(),
    "P4R" => "4-LGT PAPI ON RIGHT SIDE OF RUNWAY".into(),
    "PAPI" => "PRECISION APPROACH PATH INDICATOR".into(),
    "PNI" => "A SYSTEM OF PANELS USED FOR ALIGNMENT OF APPROACH SLOPE INDICATOR".into(),
    "PNIL" => "SYSTEM OF PANELS ON LEFT SIDE OF RUNWAY THAT MAY OR MAY NOT BE LIGHTED".into(),
    "PNIR" => "SYSTEM OF PANELS ON RIGHT SIDE OF RUNWAY THAT MAY OR MAY NOT BE LIGHTED".into(),
    "PSI" => "PULSATING/STEADY BURNING VISUAL APPROACH SLOPE INDICATOR".into(),
    "PSIL" => "PULSATING/STEADY BURNING VASI ON LEFT SIDE OF RUNWAY".into(),
    "PSIR" => "PULSATING/STEADY BURNING VASI ON RIGHT SIDE OF RUNWAY".into(),
    "PVT" => concat!(
      "PRIVATELY OWNED APPROACH SLOPE INDICATOR LIGHT SYSTEM ON A",
      "PUBLIC USE AIRPORT THAT IS INTENDED FOR PRIVATE USE ONLY"
    )
    .into(),
    "S2L" => "2-BOX SAVASI ON LEFT SIDE OF RUNWAY".into(),
    "S2R" => "2-BOX SAVASI ON RIGHT SIDE OF RUNWAY".into(),
    "SAVASI" => "SIMPLIFIED ABBREVIATED VISUAL APPROACH SLOPE INDICATOR".into(),
    "TRI" => "TRI-COLOR VISUAL APPROACH SLOPE INDICATOR".into(),
    "TRIL" => "TRI-COLOR VASI ON LEFT SIDE OF RUNWAY".into(),
    "TRIR" => "TRI-COLOR VASI ON RIGHT SIDE OF RUNWAY".into(),
    "V12" => "12-BOX VASI ON BOTH SIDES OF RUNWAY".into(),
    "V16" => "16-BOX VASI ON BOTH SIDES OF RUNWAY".into(),
    "V2L" => "2-BOX VASI ON LEFT SIDE OF RUNWAY".into(),
    "V2R" => "2-BOX VASI ON RIGHT SIDE OF RUNWAY".into(),
    "V4L" => "4-BOX VASI ON LEFT SIDE OF RUNWAY".into(),
    "V4R" => "4-BOX VASI ON RIGHT SIDE OF RUNWAY".into(),
    "V6L" => "6-BOX VASI ON LEFT SIDE OF RUNWAY".into(),
    "V6R" => "6-BOX VASI ON RIGHT SIDE OF RUNWAY".into(),
    "VAS" => "NON-SPECIFIC VASI SYSTEM".into(),
    "VASI" => "VISUAL APPROACH SLOPE INDICATOR".into(),
    _ => vgsi,
  })
}

fn get_obstacle(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let obstacle = common::get_string(feature, fields.obstn_type)?;
  let marked = common::get_string(feature, fields.obstn_mrkd_code)?;
  let marked = match marked.as_str() {
    "M" => "MARKED",
    "L" => "LIGHTED",
    "ML" => "MARKED AND LIGHTED",
    _ => Default::default(),
  };

  if obstacle.is_empty() || marked.is_empty() {
    return Some(obstacle);
  }

  Some(format!("{obstacle} ({marked})"))
}

fn get_obstacle_offset(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let distance = common::get_string(feature, fields.dist_from_thr)?;
  if distance.is_empty() {
    return Some(distance);
  }

  let offset = common::get_string(feature, fields.cntrln_offset)?;
  let direction = common::get_string(feature, fields.cntrln_dir_code)?;
  let direction = match direction.as_str() {
    "B" => Default::default(), // Missing from layout doc.
    "L" => "LEFT",
    "L/R" => "LEFT AND RIGHT",
    "R" => "RIGHT",
    _ => Default::default(),
  };

  if offset.is_empty() || offset == "0" || direction.is_empty() {
    return Some(format!("{distance} FEET"));
  }

  Some(format!("{distance} FEET, {offset} FEET {direction} OF CENTERLINE"))
}
