use crate::{
  geom,
  nasr::{apt_rmk_csv, apt_rwy_csv, cls_arsp_csv, common, frq_csv},
  ok, util,
};
use gdal::{errors, vector};
use std::{collections, path};

/// Dataset source for for `APT_BASE.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: common::IDMap,
  name_vec: Vec<(Box<str>, u64)>,
  sp_idx: rstar::RTree<LocIdx>,
}

impl Source {
  /// Open an airport base data source.
  /// - `path`: CSV zip file path
  pub fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let path = path::PathBuf::from(["/vsizip/", path.to_str().unwrap()].concat()).join("APT_BASE.csv");
    let dataset = gdal::Dataset::open_ex(path, common::open_options())?;
    let fields = Fields::new(dataset.layer(0)?)?;
    Ok(Self {
      dataset,
      fields,
      id_map: common::IDMap::new(),
      name_vec: Vec::new(),
      sp_idx: rstar::RTree::new(),
    })
  }

  /// Create the indexes.
  /// - `to_chart`: coordinate transformation and chart bounds
  /// - `cancel`: cancellation object
  pub fn create_indexes(&mut self, to_chart: &common::ToChart, cancel: &util::Cancel) {
    use vector::LayerAccess;

    let mut layer = self.layer();
    let count = layer.feature_count() as usize;

    let mut id_map = common::IDMap::with_capacity(count);
    let mut name_vec = Vec::with_capacity(count);
    let mut loc_vec = Vec::with_capacity(count);

    // Iterator resets feature reading when dropped.
    for feature in layer.features() {
      if cancel.canceled() {
        return;
      }

      if let Some(coord) = get_coord(&feature, &self.fields)
        && let Some(coord) = ok!(to_chart.transform(coord))
        && to_chart.bounds().contains(coord)
        && let Some(fid) = feature.fid()
        && let Some(id) = common::get_stack_string(&feature, self.fields.arpt_id)
        && let Some(name) = common::get_field_as_str(&feature, self.fields.arpt_name)
      {
        id_map.insert(id, fid);
        name_vec.push((name.into(), fid));
        loc_vec.push(LocIdx { coord, fid })
      };
    }

    self.id_map = id_map;
    self.name_vec = name_vec;
    self.sp_idx = rstar::RTree::bulk_load(loc_vec);
  }

  pub fn clear_indexes(&mut self) {
    self.id_map = collections::HashMap::new();
    self.name_vec = Vec::new();
    self.sp_idx = rstar::RTree::new();
  }

  /// Get airport summary information for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn airport(&self, id: &str, cancel: &util::Cancel) -> Option<Summary> {
    use vector::LayerAccess;

    let &fid = self.id_map.get(&util::StackString::from_str(id)?)?;
    if cancel.canceled() {
      return None;
    }

    let layer = util::Layer::new(self.layer());
    let feature = layer.feature(fid)?;
    Summary::new(feature, &self.fields, true)
  }

  /// Get airport detail information.
  /// - `summary`: airport summary information
  /// - `runways`: vector of runway information
  /// - `cancel`: cancellation object
  pub fn detail(
    &self,
    summary: Summary,
    frequencies: Vec<frq_csv::Frequency>,
    runways: Vec<apt_rwy_csv::Runway>,
    remarks: Vec<apt_rmk_csv::Remark>,
    airspace: Option<cls_arsp_csv::ClassAirspace>,
    cancel: &util::Cancel,
  ) -> Option<Box<Detail>> {
    use vector::LayerAccess;

    let &fid = self.id_map.get(&summary.id)?;
    if cancel.canceled() {
      return None;
    }

    let layer = util::Layer::new(self.layer());
    let feature = layer.feature(fid)?;
    Detail::new(feature, &self.fields, summary, frequencies, runways, remarks, airspace)
  }

  /// Find airports within a search radius.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  /// - `cancel`: cancellation object
  pub fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: &util::Cancel) -> Vec<Summary> {
    use vector::LayerAccess;

    let coord = [coord.x, coord.y];
    let dsq = dist * dist;

    // Get the feature IDs within the search radius.
    let mut fids: Vec<_> = self.sp_idx.locate_within_distance(coord, dsq).map(|i| i.fid).collect();
    if cancel.canceled() {
      return Vec::new();
    }

    // Sort the feature IDs so that feature lookups are sequential.
    fids.sort_unstable();

    let layer = util::Layer::new(self.layer());
    let mut airports = Vec::with_capacity(fids.len());
    for fid in fids {
      if cancel.canceled() {
        return Vec::new();
      }

      if let Some(feature) = layer.feature(fid)
        && let Some(summary) = Summary::new(feature, &self.fields, nph)
      {
        airports.push(summary);
      };
    }

    // Sort ascending by name.
    airports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    airports
  }

  /// Search for airports with names that contain the specified text.
  /// - `term`: search text
  /// - `to_chart`: coordinate transformation and chart bounds
  /// - `nph`: include non-public heliports
  /// - `cancel`: cancellation object
  pub fn search(&self, term: &str, nph: bool, cancel: &util::Cancel) -> Vec<Summary> {
    use vector::LayerAccess;

    let layer = util::Layer::new(self.layer());
    let mut airports = Vec::new();
    for (name, fid) in &self.name_vec {
      if cancel.canceled() {
        return Vec::new();
      }

      if name.contains(term)
        && let Some(feature) = layer.feature(*fid)
        && let Some(summary) = Summary::new(feature, &self.fields, nph)
      {
        airports.push(summary);
      }
    }

    // Sort ascending by name.
    airports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    airports
  }

  pub fn id_map(&self) -> &collections::HashMap<util::StackString, u64> {
    &self.id_map
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

/// Airport summary information.
pub struct Summary {
  id: util::StackString,
  name: Box<str>,
  coord: geom::DD,
  apt_type: Type,
  apt_use: Use,
}

impl Summary {
  fn new(feature: vector::Feature, fields: &Fields, nph: bool) -> Option<Self> {
    let airport_type = get_airport_type(&feature, fields)?;
    let airport_use = get_airport_use(&feature, fields)?;
    if !nph && airport_type == Type::Heliport && airport_use != Use::Public {
      return None;
    }

    Some(Self {
      id: common::get_stack_string(&feature, fields.arpt_id)?,
      name: common::get_field_as_str(&feature, fields.arpt_name)?.into(),
      coord: get_coord(&feature, fields)?,
      apt_type: airport_type,
      apt_use: airport_use,
    })
  }

  pub fn id(&self) -> &str {
    self.id.as_str()
  }

  pub fn name(&self) -> &str {
    &self.name
  }

  pub fn coord(&self) -> geom::DD {
    self.coord
  }

  pub fn get_text(&self) -> String {
    format!(
      "{} ({}), {}, {}",
      self.name,
      self.id.as_str(),
      self.apt_type.abv(),
      self.apt_use.abv()
    )
  }

  fn get_coordinates_text(&self) -> String {
    format!(
      "Coordinates: [color=white]{}, {}[/color]\n",
      self.coord.get_latitude(),
      self.coord.get_longitude()
    )
  }

  fn get_apt_type_text(&self) -> String {
    format!("Site Type: [color=white]{}[/color]\n", self.apt_type.text())
  }

  fn get_apt_use_text(&self) -> String {
    format!("Facility Use: [color=white]{}[/color]\n", self.apt_use.text())
  }
}

/// Airport type.
#[derive(Clone, Eq, Debug, PartialEq)]
enum Type {
  Airport,
  Balloon,
  Glider,
  Heliport,
  Seaplane,
  Ultralight,
}

impl Type {
  /// Airport type abbreviation.
  fn abv(&self) -> &str {
    match *self {
      Self::Airport => "A",
      Self::Balloon => "B",
      Self::Glider => "G",
      Self::Heliport => "H",
      Self::Seaplane => "S",
      Self::Ultralight => "U",
    }
  }

  /// Airport type text.
  #[allow(unused)]
  fn text(&self) -> &str {
    match *self {
      Self::Airport => "AIRPORT",
      Self::Balloon => "BALLOONPORT",
      Self::Glider => "GLIDERPORT",
      Self::Heliport => "HELIPORT",
      Self::Seaplane => "SEAPLANE BASE",
      Self::Ultralight => "ULTRALIGHT",
    }
  }
}

/// Airport use.
#[derive(Clone, Eq, Debug, PartialEq)]
enum Use {
  Private,
  Public,
}

impl Use {
  /// Airport use abbreviation.
  fn abv(&self) -> &str {
    match *self {
      Self::Private => "PVT",
      Self::Public => "PUB",
    }
  }

  /// Airport use text.
  #[allow(unused)]
  fn text(&self) -> &str {
    match *self {
      Self::Private => "PRIVATE",
      Self::Public => "PUBLIC",
    }
  }
}

/// Airport detail information.
pub struct Detail {
  summary: Summary,
  fuel_types: Box<str>,
  location: Box<str>,
  elevation: Box<str>,
  pat_alt: Box<str>,
  mag_var: Box<str>,
  lndg_fee: Box<str>,
  fss_phone: Box<str>,
  seg_circ: Box<str>,
  bcn_sked: Box<str>,
  bcn_color: Box<str>,
  lgt_sked: Box<str>,
  frequencies: Box<[frq_csv::Frequency]>,
  runways: Box<[apt_rwy_csv::Runway]>,
  remarks: Box<[apt_rmk_csv::Remark]>,
  airspace: Option<cls_arsp_csv::ClassAirspace>,
}

impl Detail {
  fn new(
    feature: vector::Feature,
    fields: &Fields,
    summary: Summary,
    frequencies: Vec<frq_csv::Frequency>,
    runways: Vec<apt_rwy_csv::Runway>,
    remarks: Vec<apt_rmk_csv::Remark>,
    airspace: Option<cls_arsp_csv::ClassAirspace>,
  ) -> Option<Box<Self>> {
    Some(Box::new(Self {
      summary,
      fuel_types: get_fuel_types(&feature, fields)?.into(),
      location: get_location(&feature, fields)?.into(),
      elevation: common::get_unit_text(&feature, "FEET ASL", fields.elev)?.into(),
      pat_alt: common::get_unit_text(&feature, "FEET AGL", fields.tpa)?.into(),
      mag_var: get_magnetic_variation(&feature, fields)?.into(),
      lndg_fee: common::get_yes_no_text(&feature, fields.lndg_fee_flag)?.into(),
      fss_phone: get_fss_phone(&feature, fields)?.into(),
      seg_circ: get_segmented_circle(&feature, fields)?.into(),
      bcn_sked: get_beacon_schedule(&feature, fields)?.into(),
      bcn_color: get_beacon_color(&feature, fields)?.into(),
      lgt_sked: get_lighting_schedule(&feature, fields)?.into(),
      frequencies: frequencies.into(),
      runways: runways.into(),
      remarks: remarks.into(),
      airspace,
    }))
  }

  pub fn summary(&self) -> &Summary {
    &self.summary
  }

  pub fn get_text(&self) -> String {
    let phone_tagger = common::PhoneTagger::new();
    let mut text = self.summary.get_apt_type_text()
      + &self.summary.get_apt_use_text()
      + &self.get_location_text()
      + &self.summary.get_coordinates_text()
      + &self.get_mag_var_text()
      + &self.get_elevation_text()
      + &self.get_pattern_altitude_text()
      + &self.get_fuel_types_text()
      + &self.get_landing_fee_text()
      + &self.get_fss_phone_text(&phone_tagger)
      + &self.get_segmented_circle_text()
      + &self.get_beacon_schedule_text()
      + &self.get_beacon_color_text()
      + &self.get_lighting_schedule_text();

    if let Some(airspace) = &self.airspace {
      text += &airspace.get_text();
    }

    if !self.frequencies.is_empty() {
      text += "\nFrequencies\n[indent]";
      for frequency in &self.frequencies {
        text += &frequency.get_text(&phone_tagger);
      }
      text += "[/indent]";
    }

    for runway in &self.runways {
      text += &runway.get_text();
    }

    if !self.remarks.is_empty() {
      text += "\nRemarks\n";
      for remark in &self.remarks {
        text += &remark.get_text(&phone_tagger);
      }
    }

    text
  }

  fn get_fuel_types_text(&self) -> String {
    if self.fuel_types.is_empty() {
      return String::new();
    }
    format!("Fuel Types: [color=white]{}[/color]\n", self.fuel_types)
  }

  fn get_location_text(&self) -> String {
    format!("Location: [color=white]{}[/color]\n", self.location)
  }

  fn get_elevation_text(&self) -> String {
    format!("Field Elevation: [color=white]{}[/color]\n", self.elevation)
  }

  fn get_pattern_altitude_text(&self) -> String {
    if self.pat_alt.is_empty() {
      return String::new();
    }
    format!("Pattern Altitude: [color=white]{}[/color]\n", self.pat_alt)
  }

  fn get_mag_var_text(&self) -> String {
    if self.mag_var.is_empty() {
      return String::new();
    }
    format!("Magnetic Variation: [color=white]{}[/color]\n", self.mag_var)
  }

  fn get_landing_fee_text(&self) -> String {
    if self.lndg_fee.is_empty() {
      return String::new();
    }
    format!("Landing Fee: [color=white]{}[/color]\n", self.lndg_fee)
  }

  fn get_fss_phone_text(&self, phone_tagger: &common::PhoneTagger) -> String {
    if self.fss_phone.is_empty() {
      return String::new();
    }
    let text = phone_tagger.process_text(&self.fss_phone);
    format!("Flight Service Station: [color=white]{text}[/color]\n")
  }

  fn get_segmented_circle_text(&self) -> String {
    if self.seg_circ.is_empty() {
      return String::new();
    }
    format!("Segmented Circle: [color=white]{}[/color]\n", self.seg_circ)
  }

  fn get_beacon_schedule_text(&self) -> String {
    if self.bcn_sked.is_empty() {
      return String::new();
    }
    format!("Beacon Schedule: [color=white]{}[/color]\n", self.bcn_sked)
  }

  fn get_beacon_color_text(&self) -> String {
    if self.bcn_color.is_empty() {
      return String::new();
    }
    format!("Beacon Color: [color=white]{}[/color]\n", self.bcn_color)
  }

  fn get_lighting_schedule_text(&self) -> String {
    if self.lgt_sked.is_empty() {
      return String::new();
    }
    format!("Lighting Schedule: [color=white]{}[/color]\n", self.lgt_sked)
  }
}

/// Field indexes for `APT_BASE.csv`.
struct Fields {
  alt_toll_free_no: usize,
  arpt_id: usize,
  arpt_name: usize,
  bcn_lens_color: usize,
  bcn_lgt_sked: usize,
  city: usize,
  elev: usize,
  facility_use_code: usize,
  fuel_types: usize,
  lat_decimal: usize,
  lgt_sked: usize,
  lndg_fee_flag: usize,
  long_decimal: usize,
  mag_hemis: usize,
  mag_varn: usize,
  seg_circle_mkr_flag: usize,
  site_type_code: usize,
  state_code: usize,
  toll_free_no: usize,
  tpa: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;
    let defn = layer.defn();
    Ok(Self {
      alt_toll_free_no: defn.field_index("ALT_TOLL_FREE_NO")?,
      arpt_id: defn.field_index("ARPT_ID")?,
      arpt_name: defn.field_index("ARPT_NAME")?,
      bcn_lens_color: defn.field_index("BCN_LENS_COLOR")?,
      bcn_lgt_sked: defn.field_index("BCN_LGT_SKED")?,
      city: defn.field_index("CITY")?,
      elev: defn.field_index("ELEV")?,
      facility_use_code: defn.field_index("FACILITY_USE_CODE")?,
      fuel_types: defn.field_index("FUEL_TYPES")?,
      lat_decimal: defn.field_index("LAT_DECIMAL")?,
      lgt_sked: defn.field_index("LGT_SKED")?,
      lndg_fee_flag: defn.field_index("LNDG_FEE_FLAG")?,
      long_decimal: defn.field_index("LONG_DECIMAL")?,
      mag_hemis: defn.field_index("MAG_HEMIS")?,
      mag_varn: defn.field_index("MAG_VARN")?,
      seg_circle_mkr_flag: defn.field_index("SEG_CIRCLE_MKR_FLAG")?,
      site_type_code: defn.field_index("SITE_TYPE_CODE")?,
      state_code: defn.field_index("STATE_CODE")?,
      toll_free_no: defn.field_index("TOLL_FREE_NO")?,
      tpa: defn.field_index("TPA")?,
    })
  }
}

fn get_airport_type(feature: &vector::Feature, fields: &Fields) -> Option<Type> {
  match common::get_field_as_str(feature, fields.site_type_code)? {
    "A" => Some(Type::Airport),
    "B" => Some(Type::Balloon),
    "C" => Some(Type::Seaplane),
    "G" => Some(Type::Glider),
    "H" => Some(Type::Heliport),
    "U" => Some(Type::Ultralight),
    _ => None,
  }
}

fn get_airport_use(feature: &vector::Feature, fields: &Fields) -> Option<Use> {
  match common::get_field_as_str(feature, fields.facility_use_code)? {
    "PR" => Some(Use::Private),
    "PU" => Some(Use::Public),
    _ => None,
  }
}

fn get_coord(feature: &vector::Feature, fields: &Fields) -> Option<geom::DD> {
  Some(geom::DD::new(
    common::get_field_as_f64(feature, fields.long_decimal)?,
    common::get_field_as_f64(feature, fields.lat_decimal)?,
  ))
}

fn get_fuel_types(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let fuel_types = common::get_field_as_str(feature, fields.fuel_types)?;

  // Make sure there's a comma and space between each fuel type.
  Some(fuel_types.split(',').map(|s| s.trim()).collect::<Vec<_>>().join(", "))
}

fn get_location(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let city = common::get_field_as_str(feature, fields.city)?;
  let state = common::get_field_as_str(feature, fields.state_code)?;
  if state.is_empty() {
    return Some(city.into());
  }

  Some(format!("{city}, {state}"))
}

fn get_magnetic_variation(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let var = common::get_field_as_str(feature, fields.mag_varn)?;
  if var.is_empty() {
    return Some(String::new());
  }

  let hem = common::get_field_as_str(feature, fields.mag_hemis)?;
  if hem.is_empty() {
    return Some(String::new());
  }

  Some(format!("{var}°{hem}"))
}

fn get_fss_phone(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let fss_phone = common::get_field_as_str(feature, fields.toll_free_no)?;
  let alt_phone = common::get_field_as_str(feature, fields.alt_toll_free_no)?;
  if !fss_phone.is_empty() && !alt_phone.is_empty() {
    return Some(format!("{fss_phone} (ALT {alt_phone})"));
  }
  Some(fss_phone.into())
}

fn get_lighting_schedule<'a>(feature: &'a vector::Feature, fields: &Fields) -> Option<&'a str> {
  let lighting_schedule = common::get_field_as_str(feature, fields.lgt_sked)?;
  Some(match lighting_schedule {
    "SS-SR" => "SUNSET-SUNRISE",
    "SEE RMK" => "SEE REMARK",
    _ => lighting_schedule,
  })
}

fn get_beacon_schedule<'a>(feature: &'a vector::Feature, fields: &Fields) -> Option<&'a str> {
  let beacon_schedule = common::get_field_as_str(feature, fields.bcn_lgt_sked)?;
  Some(match beacon_schedule {
    "SS-SR" => "SUNSET-SUNRISE",
    "SEE RMK" => "SEE REMARK",
    _ => beacon_schedule,
  })
}

fn get_beacon_color<'a>(feature: &'a vector::Feature, fields: &Fields) -> Option<&'a str> {
  let beacon_color = common::get_field_as_str(feature, fields.bcn_lens_color)?;
  Some(match beacon_color {
    "WG" => "WHITE-GREEN (LIGHTED LAND AIRPORT)",
    "WY" => "WHITE-YELLOW (LIGHTED SEAPLANE BASE)",
    "WGY" => "WHITE-GREEN-YELLOW (HELIPORT)",
    "SWG" => "SPLIT-WHITE-GREEN (LIGHTED MILITARY AIRPORT)",
    "W" => "WHITE (UNLIGHTED LAND AIRPORT)",
    "Y" => "YELLOW (UNLIGHTED SEAPLANE BASE)",
    "G" => "GREEN (LIGHTED LAND AIRPORT)",
    "N" => "NONE",
    _ => beacon_color,
  })
}

fn get_segmented_circle<'a>(feature: &'a vector::Feature, fields: &Fields) -> Option<&'a str> {
  let segmented_circle = common::get_field_as_str(feature, fields.seg_circle_mkr_flag)?;
  Some(match segmented_circle {
    "Y" => "YES",
    "N" => "NO",
    "Y-L" => "YES, LIGHTED",
    _ => segmented_circle,
  })
}

/// Location spatial index item.
struct LocIdx {
  coord: geom::Cht,
  fid: u64,
}

impl rstar::RTreeObject for LocIdx {
  type Envelope = rstar::AABB<[f64; 2]>;

  fn envelope(&self) -> Self::Envelope {
    Self::Envelope::from_point([self.coord.x, self.coord.y])
  }
}

impl rstar::PointDistance for LocIdx {
  fn distance_2(&self, point: &[f64; 2]) -> f64 {
    let dx = point[0] - self.coord.x;
    let dy = point[1] - self.coord.y;
    dx * dx + dy * dy
  }
}
