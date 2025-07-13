use crate::{
  geom,
  nasr::{apt_base_csv, apt_rmk_csv, apt_rwy_csv, apt_rwy_end_csv, cls_arsp_csv, common, frq_csv},
  util,
};
use gdal::spatial_ref;
use std::{any, cell, path, sync, thread};
use sync::{atomic, mpsc};

pub use apt_base_csv::Detail;
pub use apt_base_csv::Summary;

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
  /// - `path`: path to the CSV zip file.
  pub fn new(path: &path::Path) -> Result<Self, util::Error> {
    let sources = (
      match apt_base_csv::Source::open(path) {
        Ok(src) => src,
        Err(err) => {
          let err = format!("Unable to open airport base data source:\n{err}");
          return Err(err.into());
        }
      },
      match cls_arsp_csv::Source::open(path) {
        Ok(src) => src,
        Err(err) => {
          let err = format!("Unable to open class airspace data source:\n{err}");
          return Err(err.into());
        }
      },
      match frq_csv::Source::open(path) {
        Ok(src) => src,
        Err(err) => {
          let err = format!("Unable to open airport frequency data source:\n{err}");
          return Err(err.into());
        }
      },
      match apt_rwy_csv::Source::open(path) {
        Ok(src) => src,
        Err(err) => {
          let err = format!("Unable to open airport runway data source:\n{err}");
          return Err(err.into());
        }
      },
      match apt_rwy_end_csv::Source::open(path) {
        Ok(src) => src,
        Err(err) => {
          let err = format!("Unable to open airport runway end data source:\n{err}");
          return Err(err.into());
        }
      },
      match apt_rmk_csv::Source::open(path) {
        Ok(src) => src,
        Err(err) => {
          let err = format!("Unable to open airport remarks data source:\n{err}");
          return Err(err.into());
        }
      },
    );

    let index_status = IndexStatus::new();
    let request_count = sync::Arc::new(atomic::AtomicI32::new(0));
    let (sender, thread_receiver) = mpsc::channel();
    let (thread_sender, receiver) = mpsc::channel();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<apt_base_csv::Source>().into())
      .spawn({
        let index_status = index_status.clone();
        let request_count = request_count.clone();
        move || {
          let mut request_processor = RequestProcessor::new(sources, index_status, request_count, thread_sender);

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
  Detail(Box<Detail>),

  /// Airport summaries from a nearby search.
  Nearby(Vec<Summary>),

  /// Airport summaries matching a name/ID search.
  Search(Vec<Summary>),

  /// Request resulted in an error.
  Error(util::Error),
}

type Sources = (
  apt_base_csv::Source,
  cls_arsp_csv::Source,
  frq_csv::Source,
  apt_rwy_csv::Source,
  apt_rwy_end_csv::Source,
  apt_rmk_csv::Source,
);

struct RequestProcessor {
  base_src: apt_base_csv::Source,
  arsp_src: cls_arsp_csv::Source,
  frq_src: frq_csv::Source,
  rwy_src: apt_rwy_csv::Source,
  rwy_end_src: apt_rwy_end_csv::Source,
  rmk_src: apt_rmk_csv::Source,
  index_status: IndexStatus,
  request_count: sync::Arc<atomic::AtomicI32>,
  sender: mpsc::Sender<Reply>,
  dd_sr: spatial_ref::SpatialRef,
}

impl RequestProcessor {
  fn new(
    sources: Sources,
    index_status: IndexStatus,
    request_count: sync::Arc<atomic::AtomicI32>,
    sender: mpsc::Sender<Reply>,
  ) -> Self {
    let (base_src, arsp_src, frq_src, rwy_src, rwy_end_src, rmk_src) = sources;

    // Create a spatial reference for decimal-degree coordinates.
    // NOTE: FAA uses NAD83 for decimal-degree coordinates.
    let mut dd_sr = spatial_ref::SpatialRef::from_proj4(util::PROJ4_NAD83).unwrap();
    dd_sr.set_axis_mapping_strategy(spatial_ref::AxisMappingStrategy::TraditionalGisOrder);

    Self {
      base_src,
      arsp_src,
      frq_src,
      rwy_src,
      rwy_end_src,
      rmk_src,
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
        let reply = self.airport(&id, &cancel);
        self.send(reply, cancel);
      }
      Request::Detail(summary, cancel) => {
        let reply = self.detail(summary, &cancel);
        self.send(reply, cancel);
      }
      Request::Nearby(coord, dist, nph, cancel) => {
        let reply = self.nearby(coord, dist, nph, &cancel);
        self.send(reply, cancel);
      }
      Request::Search(term, nph, cancel) => {
        let reply = self.search(&term, nph, &cancel);
        self.send(reply, cancel);
      }
    }
  }

  fn setup_indexes(&mut self, spatial_info: Option<(String, geom::Bounds)>, cancel: util::Cancel) {
    // Clear existing indexes.
    self.index_status.reset();
    self.base_src.clear_indexes();
    self.arsp_src.clear_index();
    self.frq_src.clear_index();
    self.rwy_src.clear_index();
    self.rwy_end_src.clear_index();
    self.rmk_src.clear_index();

    let Some((proj4, bounds)) = spatial_info else {
      return;
    };

    match common::ToChart::new(&proj4, &self.dd_sr, bounds) {
      Ok(trans) => {
        self.index_status.set_has_chart_transformation();

        // Create the index needed for summary-level searches.
        self.base_src.create_indexes(&trans, &cancel);
        self.index_status.set_has_summary_index();

        // Create the indexes needed for detail-level searches.
        self.arsp_src.create_index(&self.base_src, &cancel);
        self.frq_src.create_index(&self.base_src, &cancel);
        self.rwy_src.create_index(&self.base_src, &cancel);
        self.rwy_end_src.create_index(&self.base_src, &cancel);
        self.rmk_src.create_index(&self.base_src, &cancel);
        self.index_status.set_has_detail_index();
      }
      Err(err) => {
        let reply = Reply::Error(format!("Unable to create transformation:\n{err}").into());
        self.sender.send(reply).unwrap();
      }
    }
  }

  fn airport(&self, id: &str, cancel: &util::Cancel) -> Reply {
    if !self.index_status.has_summary_index() {
      return Reply::Error("Airport summary-level indexing is required for airport ID search".into());
    }

    let id = id.trim().to_uppercase();
    if let Some(summary) = self.base_src.airport(&id, cancel) {
      return Reply::Airport(summary);
    }

    Reply::Error(format!("No airport on this chart matches ID\n'{id}'").into())
  }

  fn detail(&self, summary: Summary, cancel: &util::Cancel) -> Reply {
    if !self.index_status.has_detail_index() {
      return Reply::Error("Airport detail-level indexing is required for airport information".into());
    }

    let id = summary.id().to_owned();
    let name = summary.name().to_owned();
    let arsp = self.arsp_src.class_airspace(&id, cancel);
    let freqs = self.frq_src.frequencies(&id, cancel);
    let rwy_ends = self.rwy_end_src.runway_ends(&id, cancel);
    let rwys = self.rwy_src.runways(&id, rwy_ends, cancel);
    let rmks = self.rmk_src.remarks(&id, cancel);
    if let Some(detail) = self.base_src.detail(summary, freqs, rwys, rmks, arsp, cancel) {
      return Reply::Detail(detail);
    }

    Reply::Error(format!("Unable to get information for\n{name} ({id})").into())
  }

  fn nearby(&self, coord: geom::Cht, dist: f64, nph: bool, cancel: &util::Cancel) -> Reply {
    if !self.index_status.has_summary_index() {
      return Reply::Error("Airport summary-level indexing is required to find nearby airports".into());
    }

    Reply::Nearby(self.base_src.nearby(coord, dist, nph, cancel))
  }

  fn search(&self, term: &str, nph: bool, cancel: &util::Cancel) -> Reply {
    if !self.index_status.has_summary_index() {
      return Reply::Error("Airport summary-level indexing is required for airport search".into());
    }

    // Search for an airport ID first.
    let term = term.trim().to_uppercase();
    if let Some(summary) = self.base_src.airport(&term, cancel) {
      return Reply::Airport(summary);
    }

    // Airport ID not found, search the airport names.
    let summaries = self.base_src.search(&term, nph, cancel);
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
