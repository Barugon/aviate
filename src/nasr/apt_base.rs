use crate::{
  geom,
  nasr::{airport, common},
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
    let path = ["/vsizip/", path.to_str().unwrap()].concat();
    let path = path::Path::new(path.as_str()).join("APT_BASE.csv");
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
    use common::GetString;
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

      let Some(coord) = feature.get_coord(&self.fields) else {
        continue;
      };

      let Some(coord) = ok!(to_chart.transform(coord)) else {
        continue;
      };

      if !to_chart.bounds().contains(coord) {
        continue;
      }

      let Some(fid) = feature.fid() else {
        continue;
      };

      let Some(id) = feature.get_string(self.fields.arpt_id) else {
        continue;
      };

      let Some(name) = feature.get_string(self.fields.arpt_name) else {
        continue;
      };

      id_map.insert(id.into(), fid);
      name_vec.push((name.into(), fid));
      loc_vec.push(LocIdx { coord, fid })
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
  pub fn airport(&self, id: &str, cancel: util::Cancel) -> Option<airport::Info> {
    use vector::LayerAccess;

    let &fid = self.id_map.get(id)?;
    if cancel.canceled() {
      return None;
    }

    let layer = util::Layer::new(self.layer());
    airport::Info::new(layer.feature(fid), &self.fields, true)
  }

  /// Get airport detail information.
  /// - `info`: airport summary information
  /// - `runways`: vector of runway information
  /// - `cancel`: cancellation object
  pub fn detail(
    &self,
    info: airport::Info,
    runways: Vec<airport::Runway>,
    cancel: util::Cancel,
  ) -> Option<airport::Detail> {
    use vector::LayerAccess;

    let &fid = self.id_map.get(&info.id)?;
    if cancel.canceled() {
      return None;
    }

    let layer = util::Layer::new(self.layer());
    airport::Detail::new(layer.feature(fid), &self.fields, info, runways)
  }

  /// Find airports within a search radius.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  /// - `cancel`: cancellation object
  pub fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: util::Cancel) -> Vec<airport::Info> {
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

      let Some(info) = airport::Info::new(layer.feature(fid), &self.fields, nph) else {
        continue;
      };

      airports.push(info);
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
  pub fn search(&self, term: &str, nph: bool, cancel: util::Cancel) -> Vec<airport::Info> {
    use vector::LayerAccess;

    let layer = util::Layer::new(self.layer());
    let mut airports = Vec::new();
    for (name, fid) in &self.name_vec {
      if cancel.canceled() {
        return Vec::new();
      }

      if !name.contains(term) {
        continue;
      }

      let Some(info) = airport::Info::new(layer.feature(*fid), &self.fields, nph) else {
        continue;
      };

      airports.push(info);
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

impl airport::Info {
  fn new(feature: Option<vector::Feature>, fields: &Fields, nph: bool) -> Option<Self> {
    use common::GetString;

    let feature = feature?;
    let airport_type = feature.get_airport_type(fields)?;
    let airport_use = feature.get_airport_use(fields)?;
    if !nph && airport_type == airport::Type::Heliport && airport_use != airport::Use::Public {
      return None;
    }

    let id = feature.get_string(fields.arpt_id)?.into();
    let name = feature.get_string(fields.arpt_name)?.into();
    let coord = feature.get_coord(fields)?;

    Some(Self {
      id,
      name,
      coord,
      apt_type: airport_type,
      apt_use: airport_use,
    })
  }
}

impl airport::Detail {
  fn new(
    feature: Option<vector::Feature>,
    fields: &Fields,
    info: airport::Info,
    runways: Vec<airport::Runway>,
  ) -> Option<Self> {
    let feature = feature?;
    let runways = runways.into();
    let fuel_types = feature.get_fuel_types(fields)?.into();
    let location = feature.get_location(fields)?.into();
    let elevation = feature.get_elevation(fields)?.into();
    let pat_alt = feature.get_pattern_altitude(fields)?.into();
    let mag_var = feature.get_magnetic_variation(fields)?.into();
    Some(Self {
      info,
      fuel_types,
      location,
      elevation,
      pat_alt,
      mag_var,
      runways,
    })
  }
}

/// Field indexes for `APT_BASE.csv`.
struct Fields {
  arpt_id: usize,
  arpt_name: usize,
  site_type_code: usize,
  facility_use_code: usize,
  long_decimal: usize,
  lat_decimal: usize,
  fuel_types: usize,
  city: usize,
  state_code: usize,
  elev: usize,
  elev_method_code: usize,
  mag_varn: usize,
  mag_hemis: usize,
  tpa: usize,
}

impl Fields {
  fn new(layer: vector::Layer) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;
    let defn = layer.defn();
    Ok(Self {
      arpt_id: defn.field_index("ARPT_ID")?,
      arpt_name: defn.field_index("ARPT_NAME")?,
      site_type_code: defn.field_index("SITE_TYPE_CODE")?,
      facility_use_code: defn.field_index("FACILITY_USE_CODE")?,
      long_decimal: defn.field_index("LONG_DECIMAL")?,
      lat_decimal: defn.field_index("LAT_DECIMAL")?,
      fuel_types: defn.field_index("FUEL_TYPES")?,
      city: defn.field_index("CITY")?,
      state_code: defn.field_index("STATE_CODE")?,
      elev: defn.field_index("ELEV")?,
      elev_method_code: defn.field_index("ELEV_METHOD_CODE")?,
      mag_varn: defn.field_index("MAG_VARN")?,
      mag_hemis: defn.field_index("MAG_HEMIS")?,
      tpa: defn.field_index("TPA")?,
    })
  }
}

trait GetAirportType {
  fn get_airport_type(&self, fields: &Fields) -> Option<airport::Type>;
}

impl GetAirportType for vector::Feature<'_> {
  fn get_airport_type(&self, fields: &Fields) -> Option<airport::Type> {
    use common::GetString;

    match self.get_string(fields.site_type_code)?.as_str() {
      "A" => Some(airport::Type::Airport),
      "B" => Some(airport::Type::Balloon),
      "C" => Some(airport::Type::Seaplane),
      "G" => Some(airport::Type::Glider),
      "H" => Some(airport::Type::Heliport),
      "U" => Some(airport::Type::Ultralight),
      _ => None,
    }
  }
}

trait GetAirportUse {
  fn get_airport_use(&self, fields: &Fields) -> Option<airport::Use>;
}

impl GetAirportUse for vector::Feature<'_> {
  fn get_airport_use(&self, fields: &Fields) -> Option<airport::Use> {
    use common::GetString;

    match self.get_string(fields.facility_use_code)?.as_str() {
      "PR" => Some(airport::Use::Private),
      "PU" => Some(airport::Use::Public),
      _ => None,
    }
  }
}

trait GetCoord {
  fn get_coord(&self, fields: &Fields) -> Option<geom::DD>;
}

impl GetCoord for vector::Feature<'_> {
  fn get_coord(&self, fields: &Fields) -> Option<geom::DD> {
    use common::GetF64;

    Some(geom::DD::new(
      self.get_f64(fields.long_decimal)?,
      self.get_f64(fields.lat_decimal)?,
    ))
  }
}

trait GetFuelTypes {
  fn get_fuel_types(&self, fields: &Fields) -> Option<String>;
}

impl GetFuelTypes for vector::Feature<'_> {
  fn get_fuel_types(&self, fields: &Fields) -> Option<String> {
    use common::GetString;

    let fuel_types = self.get_string(fields.fuel_types)?;

    // Make sure there's a comma and space between each fuel type.
    Some(fuel_types.split(',').map(|s| s.trim()).collect::<Vec<_>>().join(", "))
  }
}

trait GetLocation {
  fn get_location(&self, fields: &Fields) -> Option<String>;
}

impl GetLocation for vector::Feature<'_> {
  fn get_location(&self, fields: &Fields) -> Option<String> {
    use common::GetString;

    let city = self.get_string(fields.city)?;
    let state = self.get_string(fields.state_code)?;
    if state.is_empty() {
      return Some(city);
    }

    Some(format!("{city}, {state}"))
  }
}

trait GetElevation {
  fn get_elevation(&self, fields: &Fields) -> Option<String>;
}

impl GetElevation for vector::Feature<'_> {
  fn get_elevation(&self, fields: &Fields) -> Option<String> {
    use common::GetString;

    let elevation = self.get_string(fields.elev)?;
    let method = self.get_string(fields.elev_method_code)?;
    if method.is_empty() {
      return Some(elevation);
    }

    let method = match method.as_str() {
      "E" => "EST",
      "S" => "SURV",
      _ => return None,
    };

    Some(format!("{elevation} FEET ASL ({method})"))
  }
}

trait GetPatternAltitude {
  fn get_pattern_altitude(&self, fields: &Fields) -> Option<String>;
}

impl GetPatternAltitude for vector::Feature<'_> {
  fn get_pattern_altitude(&self, fields: &Fields) -> Option<String> {
    use common::GetString;

    let pattern_altitude = self.get_string(fields.tpa)?;
    if pattern_altitude.is_empty() {
      return Some(pattern_altitude);
    }

    Some(format!("{pattern_altitude} FEET AGL"))
  }
}

trait GetMagneticVariation {
  fn get_magnetic_variation(&self, fields: &Fields) -> Option<String>;
}

impl GetMagneticVariation for vector::Feature<'_> {
  fn get_magnetic_variation(&self, fields: &Fields) -> Option<String> {
    use common::GetString;

    let var = self.get_string(fields.mag_varn)?;
    if var.is_empty() {
      return Some(String::new());
    }

    let hem = self.get_string(fields.mag_hemis)?;
    if hem.is_empty() {
      return Some(String::new());
    }

    Some(format!("{var}°{hem}"))
  }
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
