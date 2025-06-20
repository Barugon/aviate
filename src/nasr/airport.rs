use crate::{
  geom,
  nasr::{apt_base, apt_rwy, common},
  util,
};
use gdal::spatial_ref;
use std::{any, cell, path, sync, thread};
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
  /// - `path`: path to the airport CSV zip file.
  pub fn new(path: &path::Path) -> Result<Self, util::Error> {
    let base_source = match apt_base::Source::open(path) {
      Ok(source) => source,
      Err(err) => {
        let err = format!("Unable to open airport base data source:\n{err}");
        return Err(err.into());
      }
    };

    let rwy_source = match apt_rwy::Source::open(path) {
      Ok(source) => source,
      Err(err) => {
        let err = format!("Unable to open airport runway data source:\n{err}");
        return Err(err.into());
      }
    };

    let index_status = IndexStatus::new();
    let request_count = sync::Arc::new(atomic::AtomicI32::new(0));
    let (sender, thread_receiver) = mpsc::channel();
    let (thread_sender, receiver) = mpsc::channel();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<apt_base::Source>().into())
      .spawn({
        let index_status = index_status.clone();
        let request_count = request_count.clone();
        move || {
          let mut request_processor =
            RequestProcessor::new(base_source, rwy_source, index_status, request_count, thread_sender);

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
    assert!(util::MIN_FIND_CHARS == 0 || !term.is_empty());
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
  pub id: Box<str>,
  pub name: Box<str>,
  pub coord: geom::DD,
  pub apt_type: Type,
  pub apt_use: Use,
}

impl Info {
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

/// Airport runway information.
#[derive(Clone, Debug)]
#[allow(unused)]
pub struct Runway {
  pub rwy_id: Box<str>,
  pub length: u32,
  pub width: u32,
  pub lighting: Box<str>,
  pub surface: Box<str>,
  pub condition: Box<str>,
}

/// Airport detail information.
#[derive(Clone, Debug)]
#[allow(unused)]
pub struct Detail {
  pub info: Info,
  pub fuel_types: Box<str>,
  pub location: Box<str>,
  pub elevation: Box<str>,
  pub pat_alt: Box<str>,
  pub mag_var: Box<str>,
  pub runways: Box<[Runway]>,
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
  /// Airport summary information from ID search.
  Airport(Info),

  /// Airport detail information from `Info`.
  Detail(Detail),

  /// Airport summaries from a nearby search.
  Nearby(Vec<Info>),

  /// Airport summaries matching a name/ID search.
  Search(Vec<Info>),

  /// Request resulted in an error.
  Error(util::Error),
}

struct RequestProcessor {
  base_source: apt_base::Source,
  rwy_source: apt_rwy::Source,
  index_status: IndexStatus,
  request_count: sync::Arc<atomic::AtomicI32>,
  sender: mpsc::Sender<Reply>,
  dd_sr: spatial_ref::SpatialRef,
}

impl RequestProcessor {
  fn new(
    base_source: apt_base::Source,
    rwy_source: apt_rwy::Source,
    index_status: IndexStatus,
    request_count: sync::Arc<atomic::AtomicI32>,
    sender: mpsc::Sender<Reply>,
  ) -> Self {
    // Create a spatial reference for decimal-degree coordinates.
    // NOTE: FAA uses NAD83 for decimal-degree coordinates.
    let mut dd_sr = spatial_ref::SpatialRef::from_proj4(util::PROJ4_NAD83).unwrap();
    dd_sr.set_axis_mapping_strategy(spatial_ref::AxisMappingStrategy::TraditionalGisOrder);

    Self {
      base_source,
      rwy_source,
      index_status,
      request_count,
      sender,
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
    // Clear existing airport indexes.
    self.index_status.set_is_indexed(false);
    self.base_source.clear_indexes();
    self.rwy_source.clear_index();

    let Some((proj4, bounds)) = spatial_info else {
      return;
    };

    match common::ToChart::new(&proj4, &self.dd_sr, bounds) {
      Ok(trans) => {
        // Create new airport indexes.
        let indexed = self.base_source.create_indexes(&trans, cancel.clone())
          && self.rwy_source.create_index(&self.base_source, cancel);
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
    if let Some(info) = self.base_source.airport(&id, cancel) {
      return Reply::Airport(info);
    }

    Reply::Error(format!("No airport on this chart matches ID\n'{id}'").into())
  }

  fn detail(&self, info: Info, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required for airport information".into());
    }

    let id = info.id.clone();
    if let Some(runways) = self.rwy_source.runways(&id, cancel.clone()) {
      if let Some(detail) = self.base_source.detail(info, runways, cancel) {
        return Reply::Detail(detail);
      }
    }

    Reply::Error(format!("Unable to get airport information for ID\n'{id}'").into())
  }

  fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required to find nearby airports".into());
    }

    Reply::Nearby(self.base_source.nearby(coord, dist, nph, cancel))
  }

  fn search(&self, term: &str, nph: bool, cancel: util::Cancel) -> Reply {
    if !self.index_status.is_indexed() {
      return Reply::Error("Chart transformation is required for airport search".into());
    }

    // Search for an airport ID first.
    let term = term.trim().to_uppercase();
    if let Some(info) = self.base_source.airport(&term, cancel.clone()) {
      return Reply::Airport(info);
    }

    // Airport ID not found, search the airport names.
    let infos = self.base_source.search(&term, nph, cancel);
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
