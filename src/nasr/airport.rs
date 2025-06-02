use crate::{
  geom::{self, Transform},
  util,
};
use gdal::{errors, spatial_ref, vector};
use godot::global::godot_error;
use std::{any, cell, collections, path, sync, thread};
use sync::{atomic, mpsc};

/// Reader is used for opening and reading
/// [NASR 28 day subscription](https://www.faa.gov/air_traffic/flight_info/aeronav/aero_data/NASR_Subscription/) airport
/// data.
pub struct Reader {
  request_count: sync::Arc<atomic::AtomicI32>,
  index_status: IndexStatus,
  sender: mpsc::Sender<Request>,
  receiver: mpsc::Receiver<Reply>,
  cancel: cell::Cell<Option<util::Cancel>>,
}

impl Reader {
  /// Create a new airport reader.
  /// - `path`: path to the airport CSV file.
  pub fn new(path: &path::Path) -> Result<Self, util::Error> {
    let airport_source = match Source::open(path) {
      Ok(source) => source,
      Err(err) => {
        let err = format!("Unable to open airport data source:\n{err}");
        return Err(err.into());
      }
    };

    let index_status = IndexStatus::new();
    let request_count = sync::Arc::new(atomic::AtomicI32::new(0));
    let (sender, thread_receiver) = mpsc::channel();
    let (thread_sender, receiver) = mpsc::channel();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<Source>().into())
      .spawn({
        let index_status = index_status.clone();
        let request_count = request_count.clone();
        move || {
          let mut request_processor = RequestProcessor::new(airport_source, index_status, request_count, thread_sender);

          // Wait for a message. Exit when the connection is closed.
          while let Ok(request) = thread_receiver.recv() {
            request_processor.process_request(request);
          }
        }
      })
      .unwrap();

    Ok(Self {
      request_count,
      index_status,
      sender,
      receiver,
      cancel: cell::Cell::new(None),
    })
  }

  /// Returns true if the airport source is indexed.
  pub fn is_indexed(&self) -> bool {
    self.index_status.is_indexed()
  }

  /// Set the chart spatial reference using a PROJ4 string.
  /// > **NOTE**: this is required for all queries.
  /// - `proj4`: PROJ4 text
  /// - `bounds`: chart bounds.
  pub fn set_chart_spatial_ref(&self, proj4: String, bounds: geom::Bounds) {
    assert!(!proj4.is_empty());
    let cancel = self.cancel_request();
    self.send(Request::SpatialRef(Some((proj4, bounds)), cancel), false);
  }

  /// Clear the chart spatial reference.
  #[allow(unused)]
  pub fn clear_spatial_ref(&self) {
    let cancel = self.cancel_request();
    self.send(Request::SpatialRef(None, cancel), false);
  }

  /// Lookup airport summary information using it's identifier.
  /// - `id`: airport id
  #[allow(unused)]
  pub fn airport(&self, id: String) {
    assert!(!id.is_empty());
    let cancel = self.cancel_request();
    self.send(Request::Airport(id, cancel), true);
  }

  /// Lookup airport detail information.
  /// - `info`: airport summary information
  pub fn detail(&self, info: Info) {
    assert!(!info.id.is_empty());
    let cancel = self.cancel_request();
    self.send(Request::Detail(info, cancel), true);
  }

  /// Request nearby airports.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  #[allow(unused)]
  pub fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool) {
    assert!(dist >= 0.0);
    let cancel = self.cancel_request();
    self.send(Request::Nearby(coord, dist, nph, cancel), true);
  }

  /// Find an airport by ID or airport(s) by (partial) name match.
  /// - `term`: search term
  /// - `nph`: include non-public heliports
  pub fn search(&self, term: String, nph: bool) {
    assert!(!term.is_empty());
    let cancel = self.cancel_request();
    self.send(Request::Search(term, nph, cancel), true);
  }

  /// The number of pending airport requests.
  pub fn request_count(&self) -> i32 {
    self.request_count.load(atomic::Ordering::Relaxed)
  }

  /// Get the next available reply.
  pub fn get_reply(&self) -> Option<Reply> {
    self.receiver.try_recv().ok()
  }

  fn send(&self, reply: Request, inc: bool) {
    if inc {
      self.request_count.fetch_add(1, atomic::Ordering::Relaxed);
    }
    self.sender.send(reply).unwrap();
  }

  fn cancel_request(&self) -> util::Cancel {
    let cancel = util::Cancel::default();
    if let Some(mut cancel) = self.cancel.replace(Some(cancel.clone())) {
      cancel.cancel();
    }
    cancel
  }
}

impl Drop for Reader {
  fn drop(&mut self) {
    if let Some(mut cancel) = self.cancel.take() {
      cancel.cancel();
    }
  }
}

/// Airport summary information.
#[derive(Clone, Debug)]
pub struct Info {
  /// Airport ID.
  pub id: String,

  /// Airport name.
  pub name: String,

  /// Decimal-degree coordinate.
  pub coord: geom::DD,

  /// Airport type.
  pub apt_type: Type,

  /// Airport usage.
  pub apt_use: Use,
}

impl Info {
  fn new(feature: Option<&vector::Feature>, fields: &BaseFields, nph: bool) -> Option<Self> {
    let feature = feature?;
    let airport_type = feature.get_airport_type(fields)?;
    let airport_use = feature.get_airport_use(fields)?;
    if !nph && airport_type == Type::Heliport && airport_use != Use::Public {
      return None;
    }

    let id = feature.get_string(fields.arpt_id)?;
    let name = feature.get_string(fields.arpt_name)?;
    let coord = feature.get_coord(fields)?;

    Some(Self {
      id,
      name,
      coord,
      apt_type: airport_type,
      apt_use: airport_use,
    })
  }

  pub fn desc(&self) -> String {
    format!(
      "{} ({}), {}, {}",
      self.name,
      self.id,
      self.apt_type.abv(),
      self.apt_use.abv()
    )
  }
}

/// Airport detail information.
#[derive(Clone, Debug)]
#[allow(unused)]
pub struct Detail {
  pub info: Info,
  pub fuel_types: String,
  pub location: String,
  pub elevation: String,
  pub pat_alt: String,
  pub mag_var: String,
}

impl Detail {
  fn new(feature: Option<&vector::Feature>, fields: &BaseFields, info: Info) -> Option<Self> {
    let feature = feature?;
    let fuel_types = feature.get_fuel_types(fields)?;
    let location = feature.get_location(fields)?;
    let elevation = feature.get_elevation(fields)?;
    let pat_alt = feature.get_pattern_altitude(fields)?;
    let mag_var = feature.get_magnetic_variation(fields)?;
    Some(Self {
      info,
      fuel_types,
      location,
      elevation,
      pat_alt,
      mag_var,
    })
  }
}

/// Airport type.
#[derive(Clone, Eq, Debug, PartialEq)]
pub enum Type {
  Airport,
  Balloon,
  Glider,
  Heliport,
  Seaplane,
  Ultralight,
}

impl Type {
  /// Airport type abbreviation.
  pub fn abv(&self) -> &str {
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
  pub fn text(&self) -> &str {
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
pub enum Use {
  Private,
  Public,
}

impl Use {
  /// Airport use abbreviation.
  pub fn abv(&self) -> &str {
    match *self {
      Self::Private => "PVT",
      Self::Public => "PUB",
    }
  }

  /// Airport use text.
  #[allow(unused)]
  pub fn text(&self) -> &str {
    match *self {
      Self::Private => "PRIVATE",
      Self::Public => "PUBLIC",
    }
  }
}

pub enum Reply {
  /// Airport info from ID search.
  Airport(Info),

  /// Airport detail from `Info`.
  Detail(Detail),

  /// Airport infos from a nearby search.
  Nearby(Vec<Info>),

  /// Airport infos matching a name/ID search.
  Search(Vec<Info>),

  /// Request resulted in an error.
  Error(util::Error),
}

struct RequestProcessor {
  index_status: IndexStatus,
  request_count: sync::Arc<atomic::AtomicI32>,
  sender: mpsc::Sender<Reply>,
  source: Source,
  dd_sr: spatial_ref::SpatialRef,
}

impl RequestProcessor {
  fn new(
    source: Source,
    index_status: IndexStatus,
    request_count: sync::Arc<atomic::AtomicI32>,
    sender: mpsc::Sender<Reply>,
  ) -> Self {
    // Create a spatial reference for decimal-degree coordinates.
    // NOTE: FAA uses NAD83 for decimal-degree coordinates.
    let mut dd_sr = spatial_ref::SpatialRef::from_proj4(util::PROJ4_NAD83).unwrap();
    dd_sr.set_axis_mapping_strategy(spatial_ref::AxisMappingStrategy::TraditionalGisOrder);

    Self {
      index_status,
      request_count,
      sender,
      source,
      dd_sr,
    }
  }

  fn send(&self, reply: Reply, dec: bool, cancel: util::Cancel) {
    if !cancel.canceled() {
      self.sender.send(reply).unwrap();
    }

    if dec {
      assert!(self.request_count.fetch_sub(1, atomic::Ordering::Relaxed) > 0);
    }
  }

  fn process_request(&mut self, request: Request) {
    match request {
      Request::SpatialRef(spatial_info, cancel) => {
        self.setup_indexes(spatial_info, cancel);
      }
      Request::Airport(id, cancel) => {
        let reply = self.airport(&id, cancel.clone());
        self.send(reply, true, cancel);
      }
      Request::Detail(info, cancel) => {
        let reply = self.detail(info, cancel.clone());
        self.send(reply, true, cancel);
      }
      Request::Nearby(coord, dist, nph, cancel) => {
        let reply = self.nearby(coord, dist, nph, cancel.clone());
        self.send(reply, true, cancel);
      }
      Request::Search(term, nph, cancel) => {
        let reply = self.search(&term, nph, cancel.clone());
        self.send(reply, true, cancel);
      }
    }
  }

  fn setup_indexes(&mut self, spatial_info: Option<(String, geom::Bounds)>, cancel: util::Cancel) {
    // Clear airport indexes.
    self.index_status.set_is_indexed(false);
    self.source.clear_indexes();

    let Some((proj4, bounds)) = spatial_info else {
      return;
    };

    match ToChart::new(&proj4, &self.dd_sr, bounds) {
      Ok(trans) => {
        // Create new airport indexes.
        let indexed = self.source.create_indexes(&trans, cancel);
        self.index_status.set_is_indexed(indexed);
      }
      Err(err) => {
        let reply = Reply::Error(format!("Unable to create transformation:\n{err}").into());
        self.send(reply, false, cancel);
      }
    }
  }

  fn airport(&self, id: &str, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required for airport ID search".into());
    }

    let id = id.trim().to_uppercase();
    if let Some(info) = self.source.airport(&id, cancel) {
      return Reply::Airport(info);
    }
    Reply::Error(format!("No airport on this chart matches ID\n'{id}'").into())
  }

  fn detail(&self, info: Info, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required for airport ID search".into());
    }

    let id = info.id.clone();
    if let Some(detail) = self.source.detail(info, cancel) {
      return Reply::Detail(detail);
    }
    Reply::Error(format!("No airport on this chart matches ID\n'{id}'").into())
  }

  fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required to find nearby airports".into());
    }
    Reply::Nearby(self.source.nearby(coord, dist, nph, cancel))
  }

  fn search(&self, term: &str, nph: bool, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required for airport search".into());
    }

    // Search for an airport ID first.
    let term = term.trim().to_uppercase();
    if let Some(info) = self.source.airport(&term, cancel.clone()) {
      return Reply::Airport(info);
    }

    // Airport ID not found, search the airport names.
    let infos = self.source.search(&term, nph, cancel);
    if infos.is_empty() {
      return Reply::Error(format!("Nothing on this chart matches\n'{term}'").into());
    }

    Reply::Search(infos)
  }
}

enum Request {
  SpatialRef(Option<(String, geom::Bounds)>, util::Cancel),
  Airport(String, util::Cancel),
  Detail(Info, util::Cancel),
  Nearby(geom::Cht, f64, bool, util::Cancel),
  Search(String, bool, util::Cancel),
}

struct ToChart {
  /// Coordinate transformation from decimal-degree coordinates to chart coordinates.
  trans: spatial_ref::CoordTransform,

  /// Chart bounds.
  bounds: geom::Bounds,
}

impl ToChart {
  fn new(proj4: &str, dd_sr: &spatial_ref::SpatialRef, bounds: geom::Bounds) -> errors::Result<Self> {
    // Create a transformation from decimal-degree coordinates to chart coordinates.
    let chart_sr = spatial_ref::SpatialRef::from_proj4(proj4)?;
    let trans = spatial_ref::CoordTransform::new(dd_sr, &chart_sr)?;
    Ok(ToChart { trans, bounds })
  }

  fn transform(&self, coord: geom::DD) -> errors::Result<geom::Cht> {
    Ok(self.trans.transform(*coord)?.into())
  }
}

#[derive(Clone)]
struct IndexStatus {
  indexed: sync::Arc<atomic::AtomicBool>,
}

impl IndexStatus {
  fn new() -> Self {
    Self {
      indexed: sync::Arc::new(atomic::AtomicBool::new(false)),
    }
  }

  fn set_is_indexed(&mut self, indexed: bool) {
    self.indexed.store(indexed, atomic::Ordering::Relaxed);
  }

  fn is_indexed(&self) -> bool {
    self.indexed.load(atomic::Ordering::Relaxed)
  }
}

struct Source {
  dataset: gdal::Dataset,
  fields: BaseFields,
  id_map: collections::HashMap<Box<str>, u64>,
  name_vec: Vec<(Box<str>, u64)>,
  sp_idx: rstar::RTree<LocIdx>,
}

impl Source {
  fn open_options<'a>() -> gdal::DatasetOptions<'a> {
    gdal::DatasetOptions {
      open_flags: gdal::GdalOpenFlags::GDAL_OF_READONLY
        | gdal::GdalOpenFlags::GDAL_OF_VECTOR
        | gdal::GdalOpenFlags::GDAL_OF_INTERNAL,
      ..Default::default()
    }
  }

  /// Open an airport data source.
  /// - `path`: airport CSV file path
  fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let dataset = gdal::Dataset::open_ex(path, Self::open_options())?;
    let fields = BaseFields::new(&dataset)?;
    Ok(Self {
      dataset,
      fields,
      id_map: collections::HashMap::new(),
      name_vec: Vec::new(),
      sp_idx: rstar::RTree::new(),
    })
  }

  /// Create the airport indexes.
  /// - `to_chart`: coordinate transformation and chart bounds
  fn create_indexes(&mut self, to_chart: &ToChart, cancel: util::Cancel) -> bool {
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

      let Some(coord) = util::ok(to_chart.transform(coord)) else {
        continue;
      };

      if !to_chart.bounds.contains(coord) {
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

  fn clear_indexes(&mut self) {
    self.id_map = collections::HashMap::new();
    self.name_vec = Vec::new();
    self.sp_idx = rstar::RTree::new();
  }

  /// Get `Info` for the specified airport ID.
  /// - `id`: airport ID
  fn airport(&self, id: &str, cancel: util::Cancel) -> Option<Info> {
    use vector::LayerAccess;
    let mut layer = self.layer();
    let info = {
      let fid = self.id_map.get(id)?;
      if cancel.canceled() {
        return None;
      }

      Info::new(layer.feature(*fid).as_ref(), &self.fields, true)
    };

    layer.reset_feature_reading();
    info
  }

  /// Get `Detail` for the specified airport ID.
  /// - `info`: airport `Info` struct
  fn detail(&self, info: Info, cancel: util::Cancel) -> Option<Detail> {
    use vector::LayerAccess;
    let mut layer = self.layer();
    let info = {
      let fid = self.id_map.get(info.id.as_str())?;
      if cancel.canceled() {
        return None;
      }

      Detail::new(layer.feature(*fid).as_ref(), &self.fields, info)
    };

    layer.reset_feature_reading();
    info
  }

  /// Find airports within a search radius.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: util::Cancel) -> Vec<Info> {
    use vector::LayerAccess;
    let mut layer = self.layer();
    let coord = [coord.x, coord.y];
    let dsq = dist * dist;

    // Get the feature IDs within the search radius.
    let mut fids: Vec<u64> = self.sp_idx.locate_within_distance(coord, dsq).map(|i| i.fid).collect();
    if cancel.canceled() {
      return Vec::new();
    }

    // Sort the feature IDs so that feature lookups are sequential.
    fids.sort_unstable();

    let mut airports = Vec::with_capacity(fids.len());
    for fid in fids {
      if cancel.canceled() {
        layer.reset_feature_reading();
        return Vec::new();
      }

      let Some(info) = Info::new(layer.feature(fid).as_ref(), &self.fields, nph) else {
        continue;
      };

      airports.push(info);
    }

    layer.reset_feature_reading();

    // Sort ascending by name.
    airports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    airports
  }

  /// Search for airports with names that contain the specified text.
  /// - `term`: search text
  /// - `to_chart`: coordinate transformation and chart bounds
  /// - `nph`: include non-public heliports
  fn search(&self, term: &str, nph: bool, cancel: util::Cancel) -> Vec<Info> {
    use vector::LayerAccess;
    let mut layer = self.layer();
    let mut airports = Vec::new();
    for (name, fid) in &self.name_vec {
      if cancel.canceled() {
        layer.reset_feature_reading();
        return Vec::new();
      }

      if !name.contains(term) {
        continue;
      }

      let Some(info) = Info::new(layer.feature(*fid).as_ref(), &self.fields, nph) else {
        continue;
      };

      airports.push(info);
    }

    layer.reset_feature_reading();

    // Sort ascending by name.
    airports.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    airports
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

/// Field indexes for APT_BASE.
struct BaseFields {
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

impl BaseFields {
  fn new(dataset: &gdal::Dataset) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;
    let layer = dataset.layer(0)?;
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

trait GetF64 {
  fn get_f64(&self, index: usize) -> Option<f64>;
}

impl GetF64 for vector::Feature<'_> {
  fn get_f64(&self, index: usize) -> Option<f64> {
    match self.field_as_double(index) {
      Ok(val) => val,
      Err(err) => {
        godot_error!("{err}");
        None
      }
    }
  }
}

trait GetString {
  fn get_string(&self, index: usize) -> Option<String>;
}

impl GetString for vector::Feature<'_> {
  fn get_string(&self, index: usize) -> Option<String> {
    match self.field_as_string(index) {
      Ok(val) => val,
      Err(err) => {
        godot_error!("{err}");
        None
      }
    }
  }
}

trait GetType {
  fn get_airport_type(&self, fields: &BaseFields) -> Option<Type>;
}

impl GetType for vector::Feature<'_> {
  fn get_airport_type(&self, fields: &BaseFields) -> Option<Type> {
    match self.get_string(fields.site_type_code)?.as_str() {
      "A" => Some(Type::Airport),
      "B" => Some(Type::Balloon),
      "C" => Some(Type::Seaplane),
      "G" => Some(Type::Glider),
      "H" => Some(Type::Heliport),
      "U" => Some(Type::Ultralight),
      _ => None,
    }
  }
}

trait GetUse {
  fn get_airport_use(&self, fields: &BaseFields) -> Option<Use>;
}

impl GetUse for vector::Feature<'_> {
  fn get_airport_use(&self, fields: &BaseFields) -> Option<Use> {
    match self.get_string(fields.facility_use_code)?.as_str() {
      "PR" => Some(Use::Private),
      "PU" => Some(Use::Public),
      _ => None,
    }
  }
}

trait GetCoord {
  fn get_coord(&self, fields: &BaseFields) -> Option<geom::DD>;
}

impl GetCoord for vector::Feature<'_> {
  fn get_coord(&self, fields: &BaseFields) -> Option<geom::DD> {
    Some(geom::DD::new(
      self.get_f64(fields.long_decimal)?,
      self.get_f64(fields.lat_decimal)?,
    ))
  }
}

trait GetFuelTypes {
  fn get_fuel_types(&self, fields: &BaseFields) -> Option<String>;
}

impl GetFuelTypes for vector::Feature<'_> {
  fn get_fuel_types(&self, fields: &BaseFields) -> Option<String> {
    let fuel_types = self.get_string(fields.fuel_types)?;

    // Make sure there's a comma and space between each type.
    Some(fuel_types.split(',').map(|s| s.trim()).collect::<Vec<_>>().join(", "))
  }
}

trait GetLocation {
  fn get_location(&self, fields: &BaseFields) -> Option<String>;
}

impl GetLocation for vector::Feature<'_> {
  fn get_location(&self, fields: &BaseFields) -> Option<String> {
    let city = self.get_string(fields.city)?;
    let state = self.get_string(fields.state_code)?;
    Some([city, state].join(", "))
  }
}

trait GetElevation {
  fn get_elevation(&self, fields: &BaseFields) -> Option<String>;
}

impl GetElevation for vector::Feature<'_> {
  fn get_elevation(&self, fields: &BaseFields) -> Option<String> {
    let elevation = self.get_string(fields.elev)?;
    let method = match self.get_string(fields.elev_method_code)?.as_str() {
      "E" => "(EST)",
      "S" => "(SURV)",
      _ => return None,
    };
    Some([&elevation, method].join(" "))
  }
}

trait GetPatternAltitude {
  fn get_pattern_altitude(&self, fields: &BaseFields) -> Option<String>;
}

impl GetPatternAltitude for vector::Feature<'_> {
  fn get_pattern_altitude(&self, fields: &BaseFields) -> Option<String> {
    let pattern_altitude = self.get_string(fields.tpa)?;
    if pattern_altitude.is_empty() {
      return Some(pattern_altitude);
    }
    Some([&pattern_altitude, "FEET AGL"].join(" "))
  }
}

trait GetMagneticVariation {
  fn get_magnetic_variation(&self, fields: &BaseFields) -> Option<String>;
}

impl GetMagneticVariation for vector::Feature<'_> {
  fn get_magnetic_variation(&self, fields: &BaseFields) -> Option<String> {
    let var = self.get_string(fields.mag_varn)?;
    if var.is_empty() {
      return Some(String::new());
    }

    let hem = self.get_string(fields.mag_hemis)?;
    if hem.is_empty() {
      return Some(String::new());
    }

    Some([var, hem].join("°"))
  }
}
