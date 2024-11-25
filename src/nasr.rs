use crate::{geom, util};
use gdal::{errors, spatial_ref, vector};
use godot::global::godot_error;
use std::{any, collections, path, sync, thread};
use sync::{atomic, mpsc};

// NASR = National Airspace System Resources

/// AirportReader is used for opening and reading [NASR 28 day subscription](https://www.faa.gov/air_traffic/flight_info/aeronav/aero_data/NASR_Subscription/)
/// airport data.
pub struct AirportReader {
  request_count: sync::Arc<atomic::AtomicI64>,
  airport_status: AirportStatusSync,
  tx: mpsc::Sender<AirportRequest>,
  rx: mpsc::Receiver<AirportReply>,
}

impl AirportReader {
  /// Create a new NASR airport reader.
  /// - `path`: path to the airport CSV file.
  pub fn new<P: AsRef<path::Path>>(path: P) -> Result<Self, util::Error> {
    AirportReader::_new(path.as_ref())
  }

  fn _new(path: &path::Path) -> Result<Self, util::Error> {
    let mut source = match AirportSource::open(path) {
      Ok(source) => source,
      Err(err) => {
        let err = format!("Unable to open airport data source:\n{err}");
        return Err(err.into());
      }
    };

    let airport_status = AirportStatusSync::new();
    let request_count = sync::Arc::new(atomic::AtomicI64::new(0));
    let (tx, trx) = mpsc::channel();
    let (ttx, rx) = mpsc::channel();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<AirportSource>().into())
      .spawn({
        let mut airport_status = airport_status.clone();
        let request_count = request_count.clone();
        move || {
          // Create the name and ID indexes.
          if source.create_basic_indexes() {
            airport_status.set_has_basic_idx();
          }

          let nad83 = {
            let mut nad83 = spatial_ref::SpatialRef::from_epsg(4269).unwrap();
            nad83.set_axis_mapping_strategy(spatial_ref::AxisMappingStrategy::TraditionalGisOrder);
            nad83
          };

          // Chart transformation.
          let mut to_chart = None;

          let send = {
            move |reply: AirportReply, dec: bool| {
              ttx.send(reply).unwrap();
              if dec {
                assert!(request_count.fetch_sub(1, atomic::Ordering::Relaxed) > 0);
              }
            }
          };

          // Wait for a message. Exit when the connection is closed.
          while let Ok(request) = trx.recv() {
            match request {
              AirportRequest::SpatialRef(spatial_info) => {
                if airport_status.get() >= AirportIndex::Basic {
                  airport_status.set_has_basic_idx();
                  to_chart = None;

                  if let Some((proj4, bounds)) = spatial_info {
                    match spatial_ref::SpatialRef::from_proj4(&proj4) {
                      Ok(sr) => {
                        match spatial_ref::CoordTransform::new(&nad83, &sr) {
                          Ok(trans) => {
                            let trans_info = ToChart { trans, bounds };
                            // Create the airport spatial index.
                            if source.create_spatial_index(&trans_info) {
                              airport_status.set_has_spatial_idx();
                              to_chart = Some(trans_info);
                            }
                          }
                          Err(err) => {
                            let err = format!("Unable to create coordinate transformation:\n{err}");
                            send(AirportReply::Error(err.into()), false);
                          }
                        }
                      }
                      Err(err) => {
                        let err = format!("Unable to create spatial reference:\n{err}");
                        send(AirportReply::Error(err.into()), false);
                      }
                    }
                  }
                }
              }
              AirportRequest::Airport(id) => {
                let id = id.trim().to_uppercase();
                let reply = if let Some(info) = source.airport(&id) {
                  AirportReply::Airport(info)
                } else {
                  let err = format!("No airport IDs match\n'{id}'");
                  AirportReply::Error(err.into())
                };
                send(reply, true);
              }
              AirportRequest::Nearby(coord, dist, nph) => {
                let infos = source.nearby(coord, dist, nph);
                send(AirportReply::Nearby(infos), true);
              }
              AirportRequest::Search(term, nph) => {
                if let Some(to_chart) = to_chart.as_ref() {
                  let term = term.trim().to_uppercase();

                  // Search for an airport ID first.
                  let reply = if let Some(info) = source.airport(&term) {
                    if to_chart.contains(info.coord) {
                      AirportReply::Airport(info)
                    } else {
                      let err = format!("{}\nis not on this chart", info.desc);
                      AirportReply::Error(err.into())
                    }
                  } else {
                    // Airport ID not found, search the airport names.
                    let infos = source.search(&term, to_chart, nph);
                    if infos.is_empty() {
                      let err = format!("Nothing on this chart matches\n'{term}'");
                      AirportReply::Error(err.into())
                    } else {
                      AirportReply::Search(infos)
                    }
                  };
                  send(reply, true);
                } else {
                  let err = "Chart transformation is needed for search";
                  send(AirportReply::Error(err.into()), true);
                }
              }
            }
          }
        }
      })
      .unwrap();

    Ok(Self {
      request_count,
      airport_status,
      tx,
      rx,
    })
  }

  /// True if the airport source has ID and name indexes.
  #[allow(unused)]
  pub fn has_basic_idx(&self) -> bool {
    self.airport_status.get() >= AirportIndex::Basic
  }

  /// True if the airport source has a spatial index.
  #[allow(unused)]
  pub fn has_spatial_idx(&self) -> bool {
    self.airport_status.get() >= AirportIndex::Spatial
  }

  /// Get the airport index level.
  pub fn get_index_level(&self) -> AirportIndex {
    self.airport_status.get()
  }

  /// Set the chart spatial reference using a PROJ4 string.
  /// > **NOTE**: this is required for all queries other than `airport`.
  /// - `proj4`: PROJ4 text
  /// - `bounds`: Chart bounds in LCC coordinates.
  pub fn set_spatial_ref(&self, proj4: String, bounds: geom::Bounds) {
    let request = AirportRequest::SpatialRef(Some((proj4, bounds)));
    self.tx.send(request).unwrap();
  }

  #[allow(unused)]
  /// Clear the chart spatial reference.
  pub fn clear_spatial_ref(&self) {
    let request = AirportRequest::SpatialRef(None);
    self.tx.send(request).unwrap();
  }

  #[allow(unused)]
  /// Lookup airport information using it's identifier.
  /// > **NOTE**: Ignores chart boundaries and does not require a chart spatial reference.
  /// - `id`: airport id
  pub fn airport(&self, id: String) {
    if !id.is_empty() {
      self.tx.send(AirportRequest::Airport(id)).unwrap();
      self.request_count.fetch_add(1, atomic::Ordering::Relaxed);
    }
  }

  #[allow(unused)]
  /// Request nearby airports.
  /// > **NOTE**: requires a chart spatial reference.
  /// - `coord`: chart coordinate (LCC)
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  pub fn nearby(&self, coord: geom::Coord, dist: f64, nph: bool) {
    if dist >= 0.0 {
      self
        .tx
        .send(AirportRequest::Nearby(coord, dist, nph))
        .unwrap();
      self.request_count.fetch_add(1, atomic::Ordering::Relaxed);
    }
  }

  /// Find an airport by ID or airport(s) by (partial) name match.
  /// > **NOTE**: requires a chart spatial reference.
  /// - `term`: search term
  /// - `nph`: include non-public heliports
  pub fn search(&self, term: String, nph: bool) {
    if !term.is_empty() {
      self.tx.send(AirportRequest::Search(term, nph)).unwrap();
      self.request_count.fetch_add(1, atomic::Ordering::Relaxed);
    }
  }

  /// The number of pending airport requests.
  pub fn request_count(&self) -> i64 {
    self.request_count.load(atomic::Ordering::Relaxed)
  }

  /// Get the next available reply.
  pub fn get_reply(&self) -> Option<AirportReply> {
    self.rx.try_recv().ok()
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
  /// Coordinate transformation from NAD83 to LCC.
  trans: spatial_ref::CoordTransform,

  /// Chart bounds in LCC coordinates.
  bounds: geom::Bounds,
}

impl ToChart {
  /// Test if a NAD83 coordinate is contained within the chart bounds.
  fn contains(&self, nad83: geom::Coord) -> bool {
    use geom::Transform;
    match self.trans.transform(nad83) {
      Ok(lcc) => return self.bounds.contains(lcc),
      Err(err) => godot_error!("{err}"),
    }
    false
  }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub enum AirportIndex {
  None,

  /// ID and name indexes are ready.
  Basic,

  /// Spatial index is ready.
  Spatial,
}

impl From<u8> for AirportIndex {
  fn from(value: u8) -> Self {
    const NONE: u8 = AirportIndex::None as u8;
    const BASIC: u8 = AirportIndex::Basic as u8;
    const SPATIAL: u8 = AirportIndex::Spatial as u8;
    match value {
      NONE => AirportIndex::None,
      BASIC => AirportIndex::Basic,
      SPATIAL => AirportIndex::Spatial,
      _ => unreachable!(),
    }
  }
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

  fn set_has_basic_idx(&mut self) {
    self.set(AirportIndex::Basic);
  }

  fn set_has_spatial_idx(&mut self) {
    self.set(AirportIndex::Spatial);
  }

  fn set(&mut self, status: AirportIndex) {
    self.status.store(status as u8, atomic::Ordering::Relaxed);
  }

  fn get(&self) -> AirportIndex {
    self.status.load(atomic::Ordering::Relaxed).into()
  }
}

struct AirportSource {
  dataset: gdal::Dataset,
  count: u64,
  name_vec: Vec<(String, u64)>,
  id_map: collections::HashMap<String, u64>,
  sp_idx: rstar::RTree<LocIdx>,
}

impl AirportSource {
  fn open_options<'a>() -> gdal::DatasetOptions<'a> {
    gdal::DatasetOptions {
      open_flags: gdal::GdalOpenFlags::GDAL_OF_READONLY | gdal::GdalOpenFlags::GDAL_OF_VECTOR,
      ..Default::default()
    }
  }

  /// Open an airport data source.
  /// - `path`: NASR airport CSV file path
  fn open(path: &path::Path) -> Result<Self, errors::GdalError> {
    use gdal::vector::LayerAccess;

    // Open the dataset and get the layer.
    let dataset = gdal::Dataset::open_ex(path, Self::open_options())?;
    let layer = dataset.layer(0)?;
    let count = layer.feature_count();

    Ok(Self {
      dataset,
      count,
      name_vec: Vec::new(),
      id_map: collections::HashMap::new(),
      sp_idx: rstar::RTree::new(),
    })
  }

  // Create the name and ID indexes.
  fn create_basic_indexes(&mut self) -> bool {
    use vector::LayerAccess;

    let count = self.count as usize;
    let mut name_vec = Vec::with_capacity(count);
    let mut id_map = collections::HashMap::with_capacity(count);
    for feature in self.layer().features() {
      if let Some(fid) = feature.fid() {
        // Add the airport name to the name vector.
        if let Some(name) = feature.get_string(AirportInfo::AIRPORT_NAME) {
          name_vec.push((name, fid));
        }

        // Add the airport IDs to the ID index.
        if let Some(id) = feature.get_string(AirportInfo::AIRPORT_ID) {
          id_map.insert(id, fid);
        }
      }
    }

    self.name_vec = name_vec;
    self.id_map = id_map;
    !self.name_vec.is_empty() && !self.id_map.is_empty()
  }

  /// Create the spatial index.
  /// - `to_chart`: coordinate transformation and chart bounds
  fn create_spatial_index(&mut self, to_chart: &ToChart) -> bool {
    use vector::LayerAccess;

    let mut loc_vec = Vec::with_capacity(self.count as usize);
    for feature in self.layer().features() {
      if let Some(fid) = feature.fid() {
        use geom::Transform;
        if let Some(coord) = feature
          .get_coord()
          .and_then(|nad83| to_chart.trans.transform(nad83).ok())
        {
          if to_chart.bounds.contains(coord) {
            loc_vec.push(LocIdx { coord, fid })
          }
        }
      }
    }
    self.sp_idx = rstar::RTree::bulk_load(loc_vec);
    self.sp_idx.size() > 0
  }

  /// Get `AirportInfo` for the specified airport ID.
  /// - `id`: airport ID
  fn airport(&self, id: &str) -> Option<AirportInfo> {
    use vector::LayerAccess;
    let layer = self.layer();
    if let Some(fid) = self.id_map.get(id) {
      return layer.feature(*fid).and_then(AirportInfo::new);
    }
    None
  }

  /// Find airports within a search radius.
  /// > **NOTE**: requires spatial index.
  /// - `coord`: chart coordinate (LCC)
  /// - `dist`: search distance in meters
  /// - `nph`: include non-public heliports
  fn nearby(&self, coord: geom::Coord, dist: f64, nph: bool) -> Vec<AirportInfo> {
    use vector::LayerAccess;
    let layer = self.layer();
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
      if let Some(info) = layer.feature(fid).and_then(AirportInfo::new) {
        if nph || !info.non_public_heliport() {
          airports.push(info);
        }
      }
    }

    airports.sort_unstable_by(|a, b| a.desc.cmp(&b.desc));
    airports
  }

  /// Search for airports with names that contain the specified text.
  /// - `term`: search text
  /// - `to_chart`: coordinate transformation and chart bounds
  /// - `nph`: include non-public heliports
  fn search(&self, term: &str, to_chart: &ToChart, nph: bool) -> Vec<AirportInfo> {
    use vector::LayerAccess;
    let layer = self.layer();
    let mut airports = Vec::new();
    for (name, fid) in &self.name_vec {
      if name.contains(term) {
        if let Some(info) = layer.feature(*fid).and_then(AirportInfo::new) {
          // Make sure the coordinate (NAD83) is within the chart bounds.
          if (nph || !info.non_public_heliport()) && to_chart.contains(info.coord) {
            airports.push(info);
          }
        }
      }
    }

    airports.sort_unstable_by(|a, b| a.desc.cmp(&b.desc));
    airports
  }

  fn layer(&self) -> vector::Layer {
    self.dataset.layer(0).unwrap()
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

  /// Airport ID.
  pub id: String,

  /// Airport name.
  pub name: String,

  /// Coordinate in decimal degrees (NAD 83).
  pub coord: geom::Coord,

  /// Airport type.
  pub airport_type: AirportType,

  /// Airport usage.
  pub airport_use: AirportUse,

  /// Short description for UI lists.
  pub desc: String,
}

impl AirportInfo {
  fn new(feature: vector::Feature) -> Option<Self> {
    let mut info = Self {
      fid: feature.fid()?,
      id: feature.get_string(AirportInfo::AIRPORT_ID)?,
      name: feature.get_string(AirportInfo::AIRPORT_NAME)?,
      coord: feature.get_coord()?,
      airport_type: feature.get_airport_type()?,
      airport_use: feature.get_airport_use()?,
      desc: String::new(),
    };

    info.desc = format!(
      "{} ({}), {}, {}",
      info.short_name(),
      info.id,
      info.airport_type.abv(),
      info.airport_use.abv()
    );

    Some(info)
  }

  /// Returns a potentially shortened airport name.
  pub fn short_name(&self) -> &str {
    // Attempt to shorten the name by removing extra stuff.
    if let Some(name) = self.name.split(['/', '(']).next() {
      return name.trim_end();
    }
    &self.name
  }

  /// Returns true if this is a non-public heliport.
  pub fn non_public_heliport(&self) -> bool {
    self.airport_type == AirportType::Helicopter && self.airport_use != AirportUse::Public
  }

  const AIRPORT_ID: &'static str = "ARPT_ID";
  const AIRPORT_NAME: &'static str = "ARPT_NAME";
}

trait GetF64 {
  fn get_f64(&self, field: &str) -> Option<f64>;
}

impl GetF64 for vector::Feature<'_> {
  fn get_f64(&self, field: &str) -> Option<f64> {
    match self.field_as_double_by_name(field) {
      Ok(val) => val,
      Err(err) => {
        godot_error!("{err}");
        None
      }
    }
  }
}

trait GetString {
  fn get_string(&self, field: &str) -> Option<String>;
}

impl GetString for vector::Feature<'_> {
  fn get_string(&self, field: &str) -> Option<String> {
    match self.field_as_string_by_name(field) {
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
  pub fn abv(&self) -> &'static str {
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
  fn get_airport_type(&self) -> Option<AirportType>;
}

impl GetAirportType for vector::Feature<'_> {
  fn get_airport_type(&self) -> Option<AirportType> {
    match self.get_string("SITE_TYPE_CODE")?.as_str() {
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
  pub fn abv(&self) -> &'static str {
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
  fn get_airport_use(&self) -> Option<AirportUse>;
}

impl GetAirportUse for vector::Feature<'_> {
  fn get_airport_use(&self) -> Option<AirportUse> {
    match self.get_string("OWNERSHIP_TYPE_CODE")?.as_str() {
      "CG" => Some(AirportUse::CoastGuard),
      "MA" => Some(AirportUse::AirForce),
      "MN" => Some(AirportUse::Navy),
      "MR" => Some(AirportUse::Army),
      "PU" | "PR" => Some(if self.get_string("FACILITY_USE_CODE")? == "PR" {
        AirportUse::Private
      } else {
        AirportUse::Public
      }),
      _ => None,
    }
  }
}

trait GetCoord {
  fn get_coord(&self) -> Option<geom::Coord>;
}

impl GetCoord for vector::Feature<'_> {
  fn get_coord(&self) -> Option<geom::Coord> {
    Some(geom::Coord {
      x: self.get_f64("LONG_DECIMAL")?,
      y: self.get_f64("LAT_DECIMAL")?,
    })
  }
}
