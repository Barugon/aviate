use crate::{geom, util};
use core::f64;
use gdal::{errors, spatial_ref, vector};
use godot::global::godot_error;
use std::{any, collections, path, sync, thread};
use sync::{atomic, mpsc};

// NASR = National Airspace System Resources

/// AirportReader is used for opening and reading
/// [NASR 28 day subscription](https://www.faa.gov/air_traffic/flight_info/aeronav/aero_data/NASR_Subscription/) airport
/// data.
pub struct AirportReader {
  request_count: sync::Arc<atomic::AtomicI32>,
  airport_status: AirportStatusSync,
  sender: mpsc::Sender<AirportRequest>,
  receiver: mpsc::Receiver<AirportReply>,
}

impl AirportReader {
  /// Create a new NASR airport reader.
  /// - `path`: path to the airport CSV file.
  pub fn new(path: &path::Path) -> Result<Self, util::Error> {
    let airport_source = match AirportSource::open(path) {
      Ok(source) => source,
      Err(err) => {
        let err = format!("Unable to open airport data source:\n{err}");
        return Err(err.into());
      }
    };

    let airport_status = AirportStatusSync::new();
    let request_count = sync::Arc::new(atomic::AtomicI32::new(0));
    let (sender, thread_receiver) = mpsc::channel();
    let (thread_sender, receiver) = mpsc::channel();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<AirportSource>().into())
      .spawn({
        let airport_status = airport_status.clone();
        let request_count = request_count.clone();
        move || {
          let mut request_processor =
            AirportRequestProcessor::new(airport_source, airport_status, request_count, thread_sender);

          // Wait for a message. Exit when the connection is closed.
          while let Ok(request) = thread_receiver.recv() {
            request_processor.process_request(request);
          }
        }
      })
      .unwrap();

    Ok(Self {
      request_count,
      airport_status,
      sender,
      receiver,
    })
  }

  /// Get the airport index level.
  pub fn get_index_level(&self) -> AirportIndex {
    self.airport_status.get()
  }

  /// Set the chart spatial reference using a PROJ4 string.
  /// > **NOTE**: this is required for all queries other than `airport`.
  /// - `proj4`: PROJ4 text
  /// - `bounds`: chart bounds.
  pub fn set_chart_spatial_ref(&self, proj4: String, bounds: geom::Bounds) {
    let request = AirportRequest::SpatialRef(Some((proj4, bounds)));
    self.sender.send(request).unwrap();
  }

  /// Clear the chart spatial reference.
  #[allow(unused)]
  pub fn clear_spatial_ref(&self) {
    let request = AirportRequest::SpatialRef(None);
    self.sender.send(request).unwrap();
  }

  /// Lookup airport information using it's identifier.
  /// > **NOTE**: Ignores chart boundaries and does not require a chart spatial reference.
  /// - `id`: airport id
  #[allow(unused)]
  pub fn airport(&self, id: String) {
    if !id.is_empty() {
      self.request_count.fetch_add(1, atomic::Ordering::Relaxed);
      self.sender.send(AirportRequest::Airport(id)).unwrap();
    }
  }

  /// Request nearby airports.
  /// > **NOTE**: requires a chart spatial reference.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  #[allow(unused)]
  pub fn nearby(&self, coord: geom::Coord, dist: f64, nph: bool) {
    if dist >= 0.0 {
      self.request_count.fetch_add(1, atomic::Ordering::Relaxed);
      self.sender.send(AirportRequest::Nearby(coord, dist, nph)).unwrap();
    }
  }

  /// Find an airport by ID or airport(s) by (partial) name match.
  /// > **NOTE**: requires a chart spatial reference.
  /// - `term`: search term
  /// - `nph`: include non-public heliports
  pub fn search(&self, term: String, nph: bool) {
    if !term.is_empty() {
      self.request_count.fetch_add(1, atomic::Ordering::Relaxed);
      self.sender.send(AirportRequest::Search(term, nph)).unwrap();
    }
  }

  /// The number of pending airport requests.
  pub fn request_count(&self) -> i32 {
    self.request_count.load(atomic::Ordering::Relaxed)
  }

  /// Get the next available reply.
  pub fn get_reply(&self) -> Option<AirportReply> {
    self.receiver.try_recv().ok()
  }
}

struct AirportRequestProcessor {
  request_count: sync::Arc<atomic::AtomicI32>,
  sender: mpsc::Sender<AirportReply>,
  source: AirportSource,
  status: AirportStatusSync,
  dd_sr: spatial_ref::SpatialRef,
  to_chart: Option<ToChart>,
}

impl AirportRequestProcessor {
  fn new(
    mut source: AirportSource,
    mut status: AirportStatusSync,
    request_count: sync::Arc<atomic::AtomicI32>,
    sender: mpsc::Sender<AirportReply>,
  ) -> Self {
    // Create the airport basic index.
    if source.create_basic_index() {
      status.set_has_basic_index();
    }

    // Create a spatial reference for decimal-degree coordinates.
    // NOTE: FAA uses NAD83 for decimal-degree coordinates.
    let mut dd_sr = spatial_ref::SpatialRef::from_proj4(util::PROJ4_NAD83).unwrap();
    dd_sr.set_axis_mapping_strategy(spatial_ref::AxisMappingStrategy::TraditionalGisOrder);

    Self {
      request_count,
      sender,
      source,
      status,
      dd_sr,
      to_chart: None,
    }
  }

  fn send(&self, reply: AirportReply, dec: bool) {
    self.sender.send(reply).unwrap();
    if dec {
      assert!(self.request_count.fetch_sub(1, atomic::Ordering::Relaxed) > 0);
    }
  }

  fn process_request(&mut self, request: AirportRequest) {
    match request {
      AirportRequest::SpatialRef(spatial_info) => {
        self.status.set_has_basic_index();
        self.source.clear_advanced_indexes();
        self.to_chart = None;

        if let Some((proj4, bounds)) = spatial_info {
          match ToChart::new(&proj4, &self.dd_sr, bounds) {
            Ok(trans) => {
              // Create the airport advanced indexes.
              if self.source.create_advanced_indexes(&trans) {
                self.status.set_has_advanced_indexes();
                self.to_chart = Some(trans);
              }
            }
            Err(err) => {
              let reply = AirportReply::Error(format!("Unable to create transformation:\n{err}").into());
              self.send(reply, false);
            }
          }
        }
      }
      AirportRequest::Airport(id) => {
        let id = id.trim().to_uppercase();
        let reply = if let Some(info) = self.source.airport(&id) {
          AirportReply::Airport(info)
        } else {
          AirportReply::Error(format!("No airport IDs match\n'{id}'").into())
        };
        self.send(reply, true);
      }
      AirportRequest::Nearby(coord, dist, nph) => {
        let reply = if self.to_chart.is_some() {
          AirportReply::Nearby(self.source.nearby(coord, dist, nph))
        } else {
          AirportReply::Error("Chart transformation is required to find nearby airports".into())
        };
        self.send(reply, true);
      }
      AirportRequest::Search(term, nph) => {
        let reply = if let Some(to_chart) = self.to_chart.as_ref() {
          let term = term.trim().to_uppercase();

          // Search for an airport ID first.
          if let Some(info) = self.source.airport(&term) {
            // The airport ID index is not pre-filtered, so we need to check against the chart bounds.
            if to_chart.contains(info.coord) {
              AirportReply::Airport(info)
            } else {
              AirportReply::Error(format!("{}\nis not on this chart", info.desc).into())
            }
          } else {
            // Airport ID not found, search the airport names.
            let infos = self.source.search(&term, nph);
            if infos.is_empty() {
              AirportReply::Error(format!("Nothing on this chart matches\n'{term}'").into())
            } else {
              AirportReply::Search(infos)
            }
          }
        } else {
          AirportReply::Error("Chart transformation is required for airport search".into())
        };
        self.send(reply, true);
      }
    }
  }
}

enum AirportRequest {
  SpatialRef(Option<(String, geom::Bounds)>),
  Airport(String),
  Nearby(geom::Coord, f64, bool),
  Search(String, bool),
}

pub enum AirportReply {
  /// Airport info from ID search.
  Airport(AirportInfo),

  /// Airport infos from a nearby search.
  Nearby(Vec<AirportInfo>),

  /// Airport infos matching a name search.
  Search(Vec<AirportInfo>),

  /// Request resulted in an error.
  Error(util::Error),
}

struct ToChart {
  /// Coordinate transformation from decimal-degree coordinates to chart coordinates.
  trans: spatial_ref::CoordTransform,

  /// Chart bounds.
  bounds: geom::Bounds,
}

impl ToChart {
  fn new(proj4: &str, dd_sr: &spatial_ref::SpatialRef, bounds: geom::Bounds) -> Result<Self, errors::GdalError> {
    // Create a transformation from decimal-degree coordinates to chart coordinates and a bounds object.
    let chart_sr = spatial_ref::SpatialRef::from_proj4(proj4)?;
    let trans = spatial_ref::CoordTransform::new(dd_sr, &chart_sr)?;
    Ok(ToChart { trans, bounds })
  }

  /// Test if a decimal-degree coordinate is within the chart bounds.
  fn contains(&self, coord: geom::Coord) -> bool {
    use geom::Transform;

    // Convert to a chart coordinate.
    match self.trans.transform(coord) {
      Ok(coord) => return self.bounds.contains(coord),
      Err(err) => godot_error!("{err}"),
    }
    false
  }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub enum AirportIndex {
  None,

  /// Airport ID index is ready.
  Basic,

  /// Name and spatial indexes are ready.
  Advanced,
}

#[derive(Clone)]
struct AirportStatusSync {
  status: sync::Arc<atomic::AtomicU8>,
}

impl AirportStatusSync {
  fn new() -> Self {
    let status = atomic::AtomicU8::new(AirportIndex::None as u8);
    Self {
      status: sync::Arc::new(status),
    }
  }

  fn set_has_basic_index(&mut self) {
    self.set(AirportIndex::Basic);
  }

  fn set_has_advanced_indexes(&mut self) {
    self.set(AirportIndex::Advanced);
  }

  fn set(&mut self, status: AirportIndex) {
    self.status.store(status as u8, atomic::Ordering::Relaxed);
  }

  fn get(&self) -> AirportIndex {
    const NONE: u8 = AirportIndex::None as u8;
    const BASIC: u8 = AirportIndex::Basic as u8;
    const SPATIAL: u8 = AirportIndex::Advanced as u8;
    match self.status.load(atomic::Ordering::Relaxed) {
      NONE => AirportIndex::None,
      BASIC => AirportIndex::Basic,
      SPATIAL => AirportIndex::Advanced,
      _ => unreachable!(),
    }
  }
}

struct AirportSource {
  dataset: gdal::Dataset,
  indexes: AirportFieldIndexes,
  id_map: collections::HashMap<Box<str>, u64>,
  name_vec: Vec<(Box<str>, u64)>,
  sp_idx: rstar::RTree<LocIdx>,
}

impl AirportSource {
  fn open_options<'a>() -> gdal::DatasetOptions<'a> {
    gdal::DatasetOptions {
      open_flags: gdal::GdalOpenFlags::GDAL_OF_READONLY
        | gdal::GdalOpenFlags::GDAL_OF_VECTOR
        | gdal::GdalOpenFlags::GDAL_OF_INTERNAL,
      ..Default::default()
    }
  }

  /// Open an airport data source.
  /// - `path`: NASR airport CSV file path
  fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    let dataset = gdal::Dataset::open_ex(path, Self::open_options())?;
    let indexes = AirportFieldIndexes::new(&dataset)?;
    Ok(Self {
      dataset,
      indexes,
      id_map: collections::HashMap::new(),
      name_vec: Vec::new(),
      sp_idx: rstar::RTree::new(),
    })
  }

  /// Create the airport ID index.
  fn create_basic_index(&mut self) -> bool {
    use vector::LayerAccess;

    let mut layer = self.layer();
    let count = layer.feature_count();
    let mut id_map = collections::HashMap::with_capacity(count as usize);
    for feature in layer.features() {
      let Some(fid) = feature.fid() else {
        continue;
      };

      // Add the airport IDs to the ID index.
      if let Some(id) = feature.get_string(self.indexes.airport_id) {
        id_map.insert(id.into(), fid);
      }
    }

    self.id_map = id_map;
    !self.id_map.is_empty()
  }

  /// Create the name and spatial indexes.
  /// - `to_chart`: coordinate transformation and chart bounds
  fn create_advanced_indexes(&mut self, to_chart: &ToChart) -> bool {
    use geom::Transform;
    use vector::LayerAccess;

    let mut name_vec = Vec::new();
    let mut loc_vec = Vec::new();
    let mut layer = self.layer();
    for feature in layer.features() {
      let Some(fid) = feature.fid() else {
        continue;
      };

      let Some(coord) = feature.get_coord(&self.indexes) else {
        continue;
      };

      let Ok(coord) = to_chart.trans.transform(coord) else {
        continue;
      };

      if to_chart.bounds.contains(coord) {
        // Add the airport name to the name vector.
        if let Some(name) = feature.get_string(self.indexes.airport_name) {
          name_vec.push((name.into(), fid));
        }

        // Add the coordinate to the location vector.
        loc_vec.push(LocIdx { coord, fid })
      }
    }

    self.name_vec = name_vec;
    self.sp_idx = rstar::RTree::bulk_load(loc_vec);
    !self.name_vec.is_empty() && self.sp_idx.size() > 0
  }

  fn clear_advanced_indexes(&mut self) {
    self.name_vec = Vec::new();
    self.sp_idx = rstar::RTree::new();
  }

  /// Get `AirportInfo` for the specified airport ID.
  /// - `id`: airport ID
  fn airport(&self, id: &str) -> Option<AirportInfo> {
    use vector::LayerAccess;
    let mut layer = self.layer();
    let fid = self.id_map.get(id)?;
    let feature = layer.feature(*fid)?;
    let info = AirportInfo::new(feature, &self.indexes, true);
    layer.reset_feature_reading();
    info
  }

  /// Find airports within a search radius.
  /// > **NOTE**: requires advanced indexes.
  /// - `coord`: chart coordinate
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  fn nearby(&self, coord: geom::Coord, dist: f64, nph: bool) -> Vec<AirportInfo> {
    use vector::LayerAccess;
    let mut layer = self.layer();
    let coord = [coord.x, coord.y];
    let dsq = dist * dist;

    // Collect the feature IDs.
    let mut fids = Vec::new();
    for item in self.sp_idx.locate_within_distance(coord, dsq) {
      fids.push(item.fid);
    }

    // Sort the feature IDs so that lookups are sequential.
    fids.sort_unstable();

    let mut airports = Vec::with_capacity(fids.len());
    for fid in fids {
      let Some(feature) = layer.feature(fid) else {
        continue;
      };

      let Some(info) = AirportInfo::new(feature, &self.indexes, nph) else {
        continue;
      };

      airports.push(info);
    }

    layer.reset_feature_reading();
    airports.sort_unstable_by(|a, b| a.desc.cmp(&b.desc));
    airports
  }

  /// Search for airports with names that contain the specified text.
  /// > **NOTE**: requires advanced indexes.
  /// - `term`: search text
  /// - `to_chart`: coordinate transformation and chart bounds
  /// - `nph`: include non-public heliports
  fn search(&self, term: &str, nph: bool) -> Vec<AirportInfo> {
    use vector::LayerAccess;
    let mut layer = self.layer();
    let mut airports = Vec::new();
    for (name, fid) in &self.name_vec {
      if !name.contains(term) {
        continue;
      }

      let Some(feature) = layer.feature(*fid) else {
        continue;
      };

      let Some(info) = AirportInfo::new(feature, &self.indexes, nph) else {
        continue;
      };

      airports.push(info);
    }

    layer.reset_feature_reading();
    airports.sort_unstable_by(|a, b| a.desc.cmp(&b.desc));
    airports
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
  }
}

struct AirportFieldIndexes {
  airport_id: usize,
  airport_name: usize,
  site_type_code: usize,
  ownership_type_code: usize,
  facility_use_code: usize,
  long_decimal: usize,
  lat_decimal: usize,
}

impl AirportFieldIndexes {
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

/// Airport information.
#[derive(Debug)]
pub struct AirportInfo {
  /// Feature record ID.
  #[allow(unused)]
  pub fid: u64,

  /// Decimal-degree coordinate.
  pub coord: geom::Coord,

  /// Short description for UI lists.
  pub desc: Box<str>,
}

impl AirportInfo {
  fn new(feature: vector::Feature, indexes: &AirportFieldIndexes, nph: bool) -> Option<Self> {
    let airport_type = feature.get_airport_type(indexes)?;
    let airport_use = feature.get_airport_use(indexes)?;
    if !nph && airport_type == AirportType::Helicopter && airport_use != AirportUse::Public {
      return None;
    }

    let fid = feature.fid()?;
    let id = feature.get_string(indexes.airport_id)?;
    let name = feature.get_string(indexes.airport_name)?;
    let coord = feature.get_coord(indexes)?;
    let short_name = if let Some(name) = name.split(['/', '(']).next() {
      name.trim_end()
    } else {
      &name
    };

    let desc = format!("{} ({}), {}, {}", short_name, id, airport_type.abv(), airport_use.abv()).into();
    Some(Self { fid, coord, desc })
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

#[derive(Eq, Debug, PartialEq)]
pub enum AirportType {
  Airport,
  Balloon,
  Glider,
  Helicopter,
  Seaplane,
  Ultralight,
}

impl AirportType {
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

trait GetAirportType {
  fn get_airport_type(&self, indexes: &AirportFieldIndexes) -> Option<AirportType>;
}

impl GetAirportType for vector::Feature<'_> {
  fn get_airport_type(&self, indexes: &AirportFieldIndexes) -> Option<AirportType> {
    match self.get_string(indexes.site_type_code)?.as_str() {
      "A" => Some(AirportType::Airport),
      "B" => Some(AirportType::Balloon),
      "C" => Some(AirportType::Seaplane),
      "G" => Some(AirportType::Glider),
      "H" => Some(AirportType::Helicopter),
      "U" => Some(AirportType::Ultralight),
      _ => None,
    }
  }
}

#[derive(Eq, Debug, PartialEq)]
pub enum AirportUse {
  AirForce,
  Army,
  CoastGuard,
  Navy,
  Private,
  Public,
}

impl AirportUse {
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

trait GetAirportUse {
  fn get_airport_use(&self, indexes: &AirportFieldIndexes) -> Option<AirportUse>;
}

impl GetAirportUse for vector::Feature<'_> {
  fn get_airport_use(&self, indexes: &AirportFieldIndexes) -> Option<AirportUse> {
    match self.get_string(indexes.ownership_type_code)?.as_str() {
      "CG" => Some(AirportUse::CoastGuard),
      "MA" => Some(AirportUse::AirForce),
      "MN" => Some(AirportUse::Navy),
      "MR" => Some(AirportUse::Army),
      "PU" | "PR" => Some(if self.get_string(indexes.facility_use_code)? == "PR" {
        AirportUse::Private
      } else {
        AirportUse::Public
      }),
      _ => None,
    }
  }
}

trait GetCoord {
  fn get_coord(&self, indexes: &AirportFieldIndexes) -> Option<geom::Coord>;
}

impl GetCoord for vector::Feature<'_> {
  fn get_coord(&self, indexes: &AirportFieldIndexes) -> Option<geom::Coord> {
    Some(geom::Coord::new(
      self.get_f64(indexes.long_decimal)?,
      self.get_f64(indexes.lat_decimal)?,
    ))
  }
}
