use crate::{
  geom,
  nasr::{apt_rmk, apt_rwy, common},
  ok, util,
};
use gdal::{errors, vector};
use std::{collections, path};

/// Dataset source for for `APT_BASE.csv`.
pub struct Source {
  dataset: gdal::Dataset,
  fields: Fields,
  id_map: collections::HashMap<Box<str>, u64>,
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
      id_map: collections::HashMap::new(),
      name_vec: Vec::new(),
      sp_idx: rstar::RTree::new(),
    })
  }

  /// Create the indexes.
  /// - `to_chart`: coordinate transformation and chart bounds
  /// - `cancel`: cancellation object
  pub fn create_indexes(&mut self, to_chart: &common::ToChart, cancel: util::Cancel) -> bool {
    use vector::LayerAccess;

    let mut layer = self.layer();
    let count = layer.feature_count() as usize;

    let mut id_map = collections::HashMap::with_capacity(count);
    let mut name_vec = Vec::with_capacity(count);
    let mut loc_vec = Vec::with_capacity(count);

    // Iterator resets feature reading when dropped.
    for feature in layer.features() {
      if cancel.canceled() {
        return false;
      }

      if let Some(coord) = get_coord(&feature, &self.fields)
        && let Some(coord) = ok!(to_chart.transform(coord))
        && to_chart.bounds().contains(coord)
        && let Some(fid) = feature.fid()
        && let Some(id) = common::get_string(&feature, self.fields.arpt_id)
        && let Some(name) = common::get_string(&feature, self.fields.arpt_name)
      {
        id_map.insert(id.into(), fid);
        name_vec.push((name.into(), fid));
        loc_vec.push(LocIdx { coord, fid })
      };
    }

    self.id_map = id_map;
    self.name_vec = name_vec;
    self.sp_idx = rstar::RTree::bulk_load(loc_vec);
    !self.id_map.is_empty() && !self.name_vec.is_empty() && self.sp_idx.size() > 0
  }

  pub fn clear_indexes(&mut self) {
    self.id_map = collections::HashMap::new();
    self.name_vec = Vec::new();
    self.sp_idx = rstar::RTree::new();
  }

  /// Get airport summary information for the specified airport ID.
  /// - `id`: airport ID
  /// - `cancel`: cancellation object
  pub fn airport(&self, id: &str, cancel: util::Cancel) -> Option<Summary> {
    use vector::LayerAccess;

    let &fid = self.id_map.get(id)?;
    if cancel.canceled() {
      return None;
    }

    let layer = util::Layer::new(self.layer());
    Summary::new(layer.feature(fid), &self.fields, true)
  }

  /// Get airport detail information.
  /// - `summary`: airport summary information
  /// - `runways`: vector of runway information
  /// - `cancel`: cancellation object
  pub fn detail(
    &self,
    summary: Summary,
    runways: Vec<apt_rwy::Runway>,
    remarks: Vec<apt_rmk::Remark>,
    cancel: util::Cancel,
  ) -> Option<Detail> {
    use vector::LayerAccess;

    let &fid = self.id_map.get(&summary.id)?;
    if cancel.canceled() {
      return None;
    }

    let layer = util::Layer::new(self.layer());
    Detail::new(layer.feature(fid), &self.fields, summary, runways, remarks)
  }

  /// Find airports within a search radius.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  /// - `cancel`: cancellation object
  pub fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: util::Cancel) -> Vec<Summary> {
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

      if let Some(summary) = Summary::new(layer.feature(fid), &self.fields, nph) {
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
  pub fn search(&self, term: &str, nph: bool, cancel: util::Cancel) -> Vec<Summary> {
    use vector::LayerAccess;

    let layer = util::Layer::new(self.layer());
    let mut airports = Vec::new();
    for (name, fid) in &self.name_vec {
      if cancel.canceled() {
        return Vec::new();
      }

      if name.contains(term)
        && let Some(summary) = Summary::new(layer.feature(*fid), &self.fields, nph)
      {
        airports.push(summary);
      }
    }

    // Sort ascending by name.
    airports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    airports
  }

  pub fn id_map(&self) -> &collections::HashMap<Box<str>, u64> {
    &self.id_map
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

/// Airport summary information.
#[derive(Clone, Debug)]
pub struct Summary {
  id: Box<str>,
  name: Box<str>,
  coord: geom::DD,
  apt_type: Type,
  apt_use: Use,
}

impl Summary {
  fn new(feature: Option<vector::Feature>, fields: &Fields, nph: bool) -> Option<Self> {
    let feature = feature?;
    let airport_type = get_airport_type(&feature, fields)?;
    let airport_use = get_airport_use(&feature, fields)?;
    if !nph && airport_type == Type::Heliport && airport_use != Use::Public {
      return None;
    }

    let id = common::get_string(&feature, fields.arpt_id)?.into();
    let name = common::get_string(&feature, fields.arpt_name)?.into();
    let coord = get_coord(&feature, fields)?;

    Some(Self {
      id,
      name,
      coord,
      apt_type: airport_type,
      apt_use: airport_use,
    })
  }

  pub fn id(&self) -> &str {
    &self.id
  }

  pub fn coord(&self) -> geom::DD {
    self.coord
  }

  pub fn get_text(&self) -> String {
    format!(
      "{} ({}), {}, {}",
      self.name,
      self.id,
      self.apt_type.abv(),
      self.apt_use.abv()
    )
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
#[derive(Clone, Debug)]
pub struct Detail {
  summary: Summary,
  fuel_types: Box<str>,
  location: Box<str>,
  elevation: Box<str>,
  pat_alt: Box<str>,
  mag_var: Box<str>,
  lndg_fee: Box<str>,
  bcn_sked: Box<str>,
  bcn_color: Box<str>,
  lgt_sked: Box<str>,
  runways: Box<[apt_rwy::Runway]>,
  remarks: Box<[apt_rmk::Remark]>,
}

impl Detail {
  fn new(
    feature: Option<vector::Feature>,
    fields: &Fields,
    summary: Summary,
    runways: Vec<apt_rwy::Runway>,
    remarks: Vec<apt_rmk::Remark>,
  ) -> Option<Self> {
    let feature = feature?;
    let runways = runways.into();
    let remarks = remarks.into();
    let fuel_types = get_fuel_types(&feature, fields)?.into();
    let location = get_location(&feature, fields)?.into();
    let elevation = get_elevation(&feature, fields)?.into();
    let pat_alt = get_pattern_altitude(&feature, fields)?.into();
    let mag_var = get_magnetic_variation(&feature, fields)?.into();
    let lndg_fee = get_landing_fee(&feature, fields)?.into();
    let bcn_sked = get_beacon_schedule(&feature, fields)?.into();
    let bcn_color = get_beacon_color(&feature, fields)?.into();
    let lgt_sked = get_lighting_schedule(&feature, fields)?.into();
    Some(Self {
      summary,
      fuel_types,
      location,
      elevation,
      pat_alt,
      mag_var,
      lndg_fee,
      bcn_sked,
      bcn_color,
      lgt_sked,
      runways,
      remarks,
    })
  }

  pub fn summary(&self) -> &Summary {
    &self.summary
  }

  pub fn get_text(&self) -> String {
    // TODO: Frequency information.

    let mut text = format!(
      include_str!("../../res/apt_info.txt"),
      self.summary.id,
      self.summary.name,
      self.summary.apt_type.text(),
      self.summary.apt_use.text(),
      self.location,
      self.summary.coord.get_latitude(),
      self.summary.coord.get_longitude(),
      self.mag_var,
      self.elevation,
      self.pat_alt,
      self.fuel_types,
      self.lndg_fee,
      self.bcn_sked,
      self.bcn_color,
      self.lgt_sked,
    );

    for runway in &self.runways {
      text += &runway.get_text();
    }

    if !self.remarks.is_empty() {
      text += "\nRemarks\n";
      for remark in &self.remarks {
        text += &remark.get_text();
      }
    }

    text
  }
}

/// Field indexes for `APT_BASE.csv`.
struct Fields {
  arpt_id: usize,
  arpt_name: usize,
  bcn_lens_color: usize,
  bcn_lgt_sked: usize,
  city: usize,
  elev_method_code: usize,
  elev: usize,
  facility_use_code: usize,
  fuel_types: usize,
  lat_decimal: usize,
  lgt_sked: usize,
  lndg_fee_flag: usize,
  long_decimal: usize,
  mag_hemis: usize,
  mag_varn: usize,
  site_type_code: usize,
  state_code: usize,
  tpa: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;
    let defn = layer.defn();
    Ok(Self {
      arpt_id: defn.field_index("ARPT_ID")?,
      arpt_name: defn.field_index("ARPT_NAME")?,
      bcn_lens_color: defn.field_index("BCN_LENS_COLOR")?,
      bcn_lgt_sked: defn.field_index("BCN_LGT_SKED")?,
      city: defn.field_index("CITY")?,
      elev_method_code: defn.field_index("ELEV_METHOD_CODE")?,
      elev: defn.field_index("ELEV")?,
      facility_use_code: defn.field_index("FACILITY_USE_CODE")?,
      fuel_types: defn.field_index("FUEL_TYPES")?,
      lat_decimal: defn.field_index("LAT_DECIMAL")?,
      lgt_sked: defn.field_index("LGT_SKED")?,
      lndg_fee_flag: defn.field_index("LNDG_FEE_FLAG")?,
      long_decimal: defn.field_index("LONG_DECIMAL")?,
      mag_hemis: defn.field_index("MAG_HEMIS")?,
      mag_varn: defn.field_index("MAG_VARN")?,
      site_type_code: defn.field_index("SITE_TYPE_CODE")?,
      state_code: defn.field_index("STATE_CODE")?,
      tpa: defn.field_index("TPA")?,
    })
  }
}

fn get_airport_type(feature: &vector::Feature, fields: &Fields) -> Option<Type> {
  match common::get_string(feature, fields.site_type_code)?.as_str() {
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
  match common::get_string(feature, fields.facility_use_code)?.as_str() {
    "PR" => Some(Use::Private),
    "PU" => Some(Use::Public),
    _ => None,
  }
}

fn get_landing_fee(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let landing_fee = common::get_string(feature, fields.lndg_fee_flag)?;
  Some(match landing_fee.as_str() {
    "Y" => String::from("YES"),
    "N" => String::from("NO"),
    _ => landing_fee,
  })
}

fn get_coord(feature: &vector::Feature, fields: &Fields) -> Option<geom::DD> {
  Some(geom::DD::new(
    common::get_f64(feature, fields.long_decimal)?,
    common::get_f64(feature, fields.lat_decimal)?,
  ))
}

fn get_fuel_types(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let fuel_types = common::get_string(feature, fields.fuel_types)?;

  // Make sure there's a comma and space between each fuel type.
  Some(fuel_types.split(',').map(|s| s.trim()).collect::<Vec<_>>().join(", "))
}

fn get_location(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let city = common::get_string(feature, fields.city)?;
  let state = common::get_string(feature, fields.state_code)?;
  if state.is_empty() {
    return Some(city);
  }

  Some(format!("{city}, {state}"))
}

fn get_elevation(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let elevation = common::get_string(feature, fields.elev)?;
  let method = common::get_string(feature, fields.elev_method_code)?;
  if method.is_empty() {
    return Some(elevation);
  }

  let method = match method.as_str() {
    "E" => "ESTIMATED",
    "S" => "SURVEYED",
    _ => return None,
  };

  Some(format!("{elevation} FEET ASL ({method})"))
}

fn get_pattern_altitude(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let pattern_altitude = common::get_string(feature, fields.tpa)?;
  if pattern_altitude.is_empty() {
    return Some(pattern_altitude);
  }

  Some(format!("{pattern_altitude} FEET AGL"))
}

fn get_magnetic_variation(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let var = common::get_string(feature, fields.mag_varn)?;
  if var.is_empty() {
    return Some(String::new());
  }

  let hem = common::get_string(feature, fields.mag_hemis)?;
  if hem.is_empty() {
    return Some(String::new());
  }

  Some(format!("{var}°{hem}"))
}

fn get_lighting_schedule(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let lighting_schedule = common::get_string(feature, fields.lgt_sked)?;
  Some(match lighting_schedule.as_str() {
    "SS-SR" => String::from("SUNSET-SUNRISE"),
    "SEE RMK" => String::from("SEE REMARK"),
    _ => lighting_schedule,
  })
}

fn get_beacon_schedule(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let beacon_schedule = common::get_string(feature, fields.bcn_lgt_sked)?;
  Some(match beacon_schedule.as_str() {
    "SS-SR" => String::from("SUNSET-SUNRISE"),
    "SEE RMK" => String::from("SEE REMARK"),
    _ => beacon_schedule,
  })
}

fn get_beacon_color(feature: &vector::Feature, fields: &Fields) -> Option<String> {
  let beacon_color = common::get_string(feature, fields.bcn_lens_color)?;
  Some(match beacon_color.as_str() {
    "WG" => String::from("WHITE-GREEN (LIGHTED LAND AIRPORT)"),
    "WY" => String::from("WHITE-YELLOW (LIGHTED SEAPLANE BASE)"),
    "WGY" => String::from("WHITE-GREEN-YELLOW (HELIPORT)"),
    "SWG" => String::from("SPLIT-WHITE-GREEN (LIGHTED MILITARY AIRPORT)"),
    "W" => String::from("WHITE (UNLIGHTED LAND AIRPORT)"),
    "Y" => String::from("YELLOW (UNLIGHTED SEAPLANE BASE)"),
    "G" => String::from("GREEN (LIGHTED LAND AIRPORT)"),
    "N" => String::from("NONE"),
    _ => beacon_color,
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
