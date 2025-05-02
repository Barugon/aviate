use crate::{geom, util};
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
    self.cancel_request();

    let request = Request::SpatialRef(Some((proj4, bounds)), self.init_cancel());
    self.send(request, false);
  }

  /// Clear the chart spatial reference.
  #[allow(unused)]
  pub fn clear_spatial_ref(&self) {
    self.cancel_request();

    let request = Request::SpatialRef(None, self.init_cancel());
    self.send(request, false);
  }

  /// Lookup airport information using it's identifier.
  /// - `id`: airport id
  #[allow(unused)]
  pub fn airport(&self, id: String) {
    assert!(!id.is_empty());
    self.cancel_request();

    let request = Request::Airport(id, self.init_cancel());
    self.send(request, true);
  }

  /// Request nearby airports.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  #[allow(unused)]
  pub fn nearby(&self, coord: geom::Coord, dist: f64, nph: bool) {
    assert!(dist >= 0.0);
    self.cancel_request();

    let request = Request::Nearby(coord, dist, nph, self.init_cancel());
    self.send(request, true);
  }

  /// Find an airport by ID or airport(s) by (partial) name match.
  /// - `term`: search term
  /// - `nph`: include non-public heliports
  pub fn search(&self, term: String, nph: bool) {
    assert!(!term.is_empty());
    self.cancel_request();

    let request = Request::Search(term, nph, self.init_cancel());
    self.send(request, true);
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

  fn cancel_request(&self) {
    if let Some(mut cancel) = self.cancel.take() {
      cancel.cancel();
    }
  }

  fn init_cancel(&self) -> util::Cancel {
    let cancel = util::Cancel::default();
    self.cancel.set(Some(cancel.clone()));
    cancel
  }
}

impl Drop for Reader {
  fn drop(&mut self) {
    self.cancel_request();
  }
}

/// Airport information.
#[derive(Debug)]
pub struct Info {
  /// Feature record ID.
  #[allow(unused)]
  pub fid: u64,

  /// Airport ID.
  pub id: String,

  /// Airport name.
  pub name: String,

  /// Decimal-degree coordinate.
  pub coord: geom::Coord,

  // Airport type.
  pub airport_type: Type,

  // Airport usage.
  pub airport_use: Use,
}

impl Info {
  fn new(feature: Option<&vector::Feature>, fields: &Fields, nph: bool) -> Option<Self> {
    let feature = feature?;
    let airport_type = feature.get_airport_type(fields)?;
    let airport_use = feature.get_airport_use(fields)?;
    if !nph && airport_type == Type::Helicopter && airport_use != Use::Public {
      return None;
    }

    let fid = feature.fid()?;
    let id = feature.get_string(fields.airport_id)?;
    let name = feature.get_string(fields.airport_name)?;
    let coord = feature.get_coord(fields)?;

    Some(Self {
      fid,
      id,
      name,
      coord,
      airport_type,
      airport_use,
    })
  }

  pub fn desc(&self) -> String {
    format!(
      "{} ({}), {}, {}",
      self.name,
      self.id,
      self.airport_type.abv(),
      self.airport_use.abv()
    )
  }
}

/// Airport type.
#[derive(Eq, Debug, PartialEq)]
pub enum Type {
  Airport,
  Balloon,
  Glider,
  Helicopter,
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
      Self::Helicopter => "H",
      Self::Seaplane => "S",
      Self::Ultralight => "U",
    }
  }
}

/// Airport use.
#[derive(Eq, Debug, PartialEq)]
pub enum Use {
  AirForce,
  Army,
  CoastGuard,
  Navy,
  Private,
  Public,
}

impl Use {
  /// Airport use abbreviation.
  pub fn abv(&self) -> &str {
    match *self {
      Self::AirForce => "USAF",
      Self::Army => "ARMY",
      Self::CoastGuard => "USCG",
      Self::Navy => "USN",
      Self::Private => "PVT",
      Self::Public => "PUB",
    }
  }
}

pub enum Reply {
  /// Airport info from ID search.
  Airport(Info),

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
        let reply = self.airport_query(&id, cancel.clone());
        self.send(reply, true, cancel);
      }
      Request::Nearby(coord, dist, nph, cancel) => {
        let reply = self.nearby_query(coord, dist, nph, cancel.clone());
        self.send(reply, true, cancel);
      }
      Request::Search(term, nph, cancel) => {
        let reply = self.search_query(&term, nph, cancel.clone());
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

  fn airport_query(&self, id: &str, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required for airport ID search".into());
    }

    let id = id.trim().to_uppercase();
    if let Some(info) = self.source.airport(&id, cancel) {
      return Reply::Airport(info);
    }
    Reply::Error(format!("No airport on this chart matches ID\n'{id}'").into())
  }

  fn nearby_query(&self, coord: geom::Coord, dist: f64, nph: bool, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required to find nearby airports".into());
    }
    Reply::Nearby(self.source.nearby(coord, dist, nph, cancel))
  }

  fn search_query(&self, term: &str, nph: bool, cancel: util::Cancel) -> Reply {
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
  Nearby(geom::Coord, f64, bool, util::Cancel),
  Search(String, bool, util::Cancel),
}

struct ToChart {
  /// Coordinate transformation from decimal-degree coordinates to chart coordinates.
  trans: spatial_ref::CoordTransform,

  /// Chart bounds.
  bounds: geom::Bounds,
}

impl ToChart {
  fn new(proj4: &str, dd_sr: &spatial_ref::SpatialRef, bounds: geom::Bounds) -> Result<Self, errors::GdalError> {
    // Create a transformation from decimal-degree coordinates to chart coordinates.
    let chart_sr = spatial_ref::SpatialRef::from_proj4(proj4)?;
    let trans = spatial_ref::CoordTransform::new(dd_sr, &chart_sr)?;
    Ok(ToChart { trans, bounds })
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
  fields: Fields,
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
    let fields = Fields::new(&dataset)?;
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
    use geom::Transform;
    use vector::LayerAccess;

    let mut id_map = collections::HashMap::new();
    let mut name_vec = Vec::new();
    let mut loc_vec = Vec::new();
    let mut layer = self.layer();

    for feature in layer.features() {
      if cancel.canceled() {
        return false;
      }

      let Some(coord) = feature.get_coord(&self.fields) else {
        continue;
      };

      let Some(coord) = util::ok(to_chart.trans.transform(coord)) else {
        continue;
      };

      if !to_chart.bounds.contains(coord) {
        continue;
      }

      let Some(fid) = feature.fid() else {
        continue;
      };

      let Some(id) = feature.get_string(self.fields.airport_id) else {
        continue;
      };

      let Some(name) = feature.get_string(self.fields.airport_name) else {
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

  /// Find airports within a search radius.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  fn nearby(&self, coord: geom::Coord, dist: f64, nph: bool, cancel: util::Cancel) -> Vec<Info> {
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

struct Fields {
  airport_id: usize,
  airport_name: usize,
  site_type_code: usize,
  ownership_type_code: usize,
  facility_use_code: usize,
  long_decimal: usize,
  lat_decimal: usize,
}

impl Fields {
  fn new(dataset: &gdal::Dataset) -> Result<Self, errors::GdalError> {
    use vector::LayerAccess;
    let layer = dataset.layer(0)?;
    let defn = layer.defn();
    Ok(Self {
      airport_id: defn.field_index("ARPT_ID")?,
      airport_name: defn.field_index("ARPT_NAME")?,
      site_type_code: defn.field_index("SITE_TYPE_CODE")?,
      ownership_type_code: defn.field_index("OWNERSHIP_TYPE_CODE")?,
      facility_use_code: defn.field_index("FACILITY_USE_CODE")?,
      long_decimal: defn.field_index("LONG_DECIMAL")?,
      lat_decimal: defn.field_index("LAT_DECIMAL")?,
    })
  }
}

/// Location spatial index item.
struct LocIdx {
  coord: geom::Coord,
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
  fn get_airport_type(&self, fields: &Fields) -> Option<Type>;
}

impl GetType for vector::Feature<'_> {
  fn get_airport_type(&self, fields: &Fields) -> Option<Type> {
    match self.get_string(fields.site_type_code)?.as_str() {
      "A" => Some(Type::Airport),
      "B" => Some(Type::Balloon),
      "C" => Some(Type::Seaplane),
      "G" => Some(Type::Glider),
      "H" => Some(Type::Helicopter),
      "U" => Some(Type::Ultralight),
      _ => None,
    }
  }
}

trait GetUse {
  fn get_airport_use(&self, fields: &Fields) -> Option<Use>;
}

impl GetUse for vector::Feature<'_> {
  fn get_airport_use(&self, fields: &Fields) -> Option<Use> {
    match self.get_string(fields.ownership_type_code)?.as_str() {
      "CG" => Some(Use::CoastGuard),
      "MA" => Some(Use::AirForce),
      "MN" => Some(Use::Navy),
      "MR" => Some(Use::Army),
      "PU" | "PR" => Some(if self.get_string(fields.facility_use_code)? == "PR" {
        Use::Private
      } else {
        Use::Public
      }),
      _ => None,
    }
  }
}

trait GetCoord {
  fn get_coord(&self, fields: &Fields) -> Option<geom::Coord>;
}

impl GetCoord for vector::Feature<'_> {
  fn get_coord(&self, fields: &Fields) -> Option<geom::Coord> {
    Some(geom::Coord::new(
      self.get_f64(fields.long_decimal)?,
      self.get_f64(fields.lat_decimal)?,
    ))
  }
}
