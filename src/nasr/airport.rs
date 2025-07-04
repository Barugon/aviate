use crate::{
  geom,
  nasr::{apt_base, apt_rmk, apt_rwy, common, frq},
  util,
};
use gdal::spatial_ref;
use std::{any, cell, path, sync, thread};
use sync::{atomic, mpsc};

pub use apt_base::Detail;
pub use apt_base::Summary;

/// Reader is used for opening and reading
/// [NASR 28 day subscription](https://www.faa.gov/air_traffic/flight_info/aeronav/aero_data/NASR_Subscription/) airport
/// data.
pub struct Reader {
  request_count: sync::Arc<atomic::AtomicI32>,
  index_status: IndexStatus,
  sender: mpsc::Sender<Request>,
  receiver: mpsc::Receiver<Reply>,
  request_cancel: cell::Cell<Option<util::Cancel>>,
  index_cancel: cell::Cell<Option<util::Cancel>>,
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

    let frq_source = match frq::Source::open(path) {
      Ok(source) => source,
      Err(err) => {
        let err = format!("Unable to open airport frequency data source:\n{err}");
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

    let rmk_source = match apt_rmk::Source::open(path) {
      Ok(source) => source,
      Err(err) => {
        let err = format!("Unable to open airport remarks data source:\n{err}");
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
          let mut request_processor = RequestProcessor::new(
            base_source,
            frq_source,
            rwy_source,
            rmk_source,
            index_status,
            request_count,
            thread_sender,
          );

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
      request_cancel: cell::Cell::new(None),
      index_cancel: cell::Cell::new(None),
    })
  }

  /// Returns true if the airport source has a chart transformation.
  pub fn has_chart_transformation(&self) -> bool {
    self.index_status.has_chart_transformation()
  }

  /// Set the chart spatial reference using a PROJ4 string.
  /// > **NOTE**: this is required for all queries.
  /// - `proj4`: PROJ4 text
  /// - `bounds`: chart bounds.
  pub fn set_chart_spatial_ref(&self, proj4: String, bounds: geom::Bounds) {
    assert!(!proj4.is_empty());
    let cancel = self.cancel_indexing();
    self.send(Request::SpatialRef(Some((proj4, bounds)), cancel), false);
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
  /// - `summary`: airport summary information
  pub fn detail(&self, summary: Summary) {
    assert!(!summary.id().is_empty());
    let cancel = self.cancel_request();
    self.send(Request::Detail(summary, cancel), true);
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
    if let Some(mut cancel) = self.request_cancel.replace(Some(cancel.clone())) {
      cancel.cancel();
    }
    cancel
  }

  fn cancel_indexing(&self) -> util::Cancel {
    let cancel = util::Cancel::default();
    if let Some(mut cancel) = self.index_cancel.replace(Some(cancel.clone())) {
      cancel.cancel();
    }
    cancel
  }
}

impl Drop for Reader {
  fn drop(&mut self) {
    if let Some(mut cancel) = self.request_cancel.take() {
      cancel.cancel();
    }

    if let Some(mut cancel) = self.index_cancel.take() {
      cancel.cancel();
    }
  }
}

pub enum Reply {
  /// Airport summary information from ID search.
  Airport(Summary),

  /// Airport detail information from `Info`.
  Detail(Detail),

  /// Airport summaries from a nearby search.
  Nearby(Vec<Summary>),

  /// Airport summaries matching a name/ID search.
  Search(Vec<Summary>),

  /// Request resulted in an error.
  Error(util::Error),
}

struct RequestProcessor {
  base_source: apt_base::Source,
  frq_source: frq::Source,
  rwy_source: apt_rwy::Source,
  rmk_source: apt_rmk::Source,
  index_status: IndexStatus,
  request_count: sync::Arc<atomic::AtomicI32>,
  sender: mpsc::Sender<Reply>,
  dd_sr: spatial_ref::SpatialRef,
}

impl RequestProcessor {
  fn new(
    base_source: apt_base::Source,
    frq_source: frq::Source,
    rwy_source: apt_rwy::Source,
    rmk_source: apt_rmk::Source,
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
      frq_source,
      rwy_source,
      rmk_source,
      index_status,
      request_count,
      sender,
      dd_sr,
    }
  }

  fn send(&self, reply: Reply, cancel: util::Cancel) {
    if !cancel.canceled() {
      self.sender.send(reply).unwrap();
    }
    assert!(self.request_count.fetch_sub(1, atomic::Ordering::Relaxed) > 0);
  }

  fn process_request(&mut self, request: Request) {
    match request {
      Request::SpatialRef(spatial_info, cancel) => {
        self.setup_indexes(spatial_info, cancel);
      }
      Request::Airport(id, cancel) => {
        let reply = self.airport(&id, cancel.clone());
        self.send(reply, cancel);
      }
      Request::Detail(summary, cancel) => {
        let reply = self.detail(summary, cancel.clone());
        self.send(reply, cancel);
      }
      Request::Nearby(coord, dist, nph, cancel) => {
        let reply = self.nearby(coord, dist, nph, cancel.clone());
        self.send(reply, cancel);
      }
      Request::Search(term, nph, cancel) => {
        let reply = self.search(&term, nph, cancel.clone());
        self.send(reply, cancel);
      }
    }
  }

  fn setup_indexes(&mut self, spatial_info: Option<(String, geom::Bounds)>, cancel: util::Cancel) {
    // Clear existing airport indexes.
    self.index_status.reset();
    self.base_source.clear_indexes();
    self.frq_source.clear_index();
    self.rwy_source.clear_index();
    self.rmk_source.clear_index();

    let Some((proj4, bounds)) = spatial_info else {
      return;
    };

    match common::ToChart::new(&proj4, &self.dd_sr, bounds) {
      Ok(trans) => {
        self.index_status.set_has_chart_transformation();

        // Create new airport indexes.
        if !self.base_source.create_indexes(&trans, cancel.clone()) {
          let reply = Reply::Error("Failed to create airport summary-level indexing".into());
          self.sender.send(reply).unwrap();
          return;
        }

        self.index_status.set_has_summary_index();

        if !self.frq_source.create_index(&self.base_source, cancel.clone())
          || !self.rwy_source.create_index(&self.base_source, cancel.clone())
          || !self.rmk_source.create_index(&self.base_source, cancel)
        {
          let reply = Reply::Error("Failed to create airport detail-level indexing".into());
          self.sender.send(reply).unwrap();
          return;
        }

        self.index_status.set_has_detail_index();
      }
      Err(err) => {
        let reply = Reply::Error(format!("Unable to create transformation:\n{err}").into());
        self.sender.send(reply).unwrap();
      }
    }
  }

  fn airport(&self, id: &str, cancel: util::Cancel) -> Reply {
    if !self.index_status.has_summary_index() {
      return Reply::Error("Airport summary-level indexing is required for airport ID search".into());
    }

    let id = id.trim().to_uppercase();
    if let Some(summary) = self.base_source.airport(&id, cancel) {
      return Reply::Airport(summary);
    }

    Reply::Error(format!("No airport on this chart matches ID\n'{id}'").into())
  }

  fn detail(&self, summary: Summary, cancel: util::Cancel) -> Reply {
    if !self.index_status.has_detail_index() {
      return Reply::Error("Airport detail-level indexing is required for airport information".into());
    }

    let id = summary.id().to_owned();
    if let Some(frequencies) = self.frq_source.frequencies(&id, cancel.clone())
      && let Some(runways) = self.rwy_source.runways(&id, cancel.clone())
      && let Some(remarks) = self.rmk_source.remarks(&id, cancel.clone())
      && let Some(detail) = self.base_source.detail(summary, frequencies, runways, remarks, cancel)
    {
      return Reply::Detail(detail);
    }

    Reply::Error(format!("Unable to get airport detail information for ID\n'{id}'").into())
  }

  fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: util::Cancel) -> Reply {
    if !self.index_status.has_summary_index() {
      return Reply::Error("Airport summary-level indexing is required to find nearby airports".into());
    }

    Reply::Nearby(self.base_source.nearby(coord, dist, nph, cancel))
  }

  fn search(&self, term: &str, nph: bool, cancel: util::Cancel) -> Reply {
    if !self.index_status.has_summary_index() {
      return Reply::Error("Airport summary-level indexing is required for airport search".into());
    }

    // Search for an airport ID first.
    let term = term.trim().to_uppercase();
    if let Some(summary) = self.base_source.airport(&term, cancel.clone()) {
      return Reply::Airport(summary);
    }

    // Airport ID not found, search the airport names.
    let summaries = self.base_source.search(&term, nph, cancel);
    if summaries.is_empty() {
      return Reply::Error(format!("Nothing on this chart matches\n'{term}'").into());
    }

    Reply::Search(summaries)
  }
}

enum Request {
  SpatialRef(Option<(String, geom::Bounds)>, util::Cancel),
  Airport(String, util::Cancel),
  Detail(Summary, util::Cancel),
  Nearby(geom::Cht, f64, bool, util::Cancel),
  Search(String, bool, util::Cancel),
}

#[derive(Clone)]
struct IndexStatus {
  index_type: sync::Arc<atomic::AtomicU8>,
}

impl IndexStatus {
  const NONE: u8 = 0;
  const TRANSFORM: u8 = 1;
  const SUMMARY: u8 = 2;
  const DETAIL: u8 = 3;

  fn new() -> Self {
    Self {
      index_type: sync::Arc::new(atomic::AtomicU8::new(IndexStatus::NONE)),
    }
  }

  fn reset(&mut self) {
    self.index_type.store(IndexStatus::NONE, atomic::Ordering::Relaxed);
  }

  fn set_has_chart_transformation(&mut self) {
    self.index_type.store(IndexStatus::TRANSFORM, atomic::Ordering::Relaxed);
  }

  fn set_has_summary_index(&mut self) {
    self.index_type.store(IndexStatus::SUMMARY, atomic::Ordering::Relaxed);
  }

  fn set_has_detail_index(&mut self) {
    self.index_type.store(IndexStatus::DETAIL, atomic::Ordering::Relaxed);
  }

  fn has_chart_transformation(&self) -> bool {
    self.index_type.load(atomic::Ordering::Relaxed) >= IndexStatus::TRANSFORM
  }

  fn has_summary_index(&self) -> bool {
    self.index_type.load(atomic::Ordering::Relaxed) >= IndexStatus::SUMMARY
  }

  fn has_detail_index(&self) -> bool {
    self.index_type.load(atomic::Ordering::Relaxed) >= IndexStatus::DETAIL
  }
}
