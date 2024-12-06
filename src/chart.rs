use crate::{config, geom, util};
use gdal::{raster, spatial_ref};
use std::{any, hash::Hash, path, sync::mpsc, thread};

/// RasterReader is used for opening and reading [VFR charts](https://www.faa.gov/air_traffic/flight_info/aeronav/digital_products/vfr/)
///  in zipped GEO-TIFF format.
pub struct RasterReader {
  transformation: Transformation,
  tx: mpsc::Sender<ImagePart>,
  rx: mpsc::Receiver<RasterReply>,
}

impl RasterReader {
  /// Create a new chart raster reader.
  /// - `path`: chart file path
  pub fn new<P: AsRef<path::Path>>(path: P) -> Result<Self, util::Error> {
    RasterReader::_new(path.as_ref())
  }

  fn _new(path: &path::Path) -> Result<Self, util::Error> {
    // Open the chart source.
    let (source, transformation, palette) = RasterSource::open(path)?;

    // Create the communication channels.
    let (tx, trx) = mpsc::channel();
    let (ttx, rx) = mpsc::channel();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<RasterReader>().to_owned())
      .spawn(move || {
        let (light, dark) = {
          // Convert the color palette.
          let mut light = Vec::with_capacity(u8::MAX as usize);
          let mut dark = Vec::with_capacity(u8::MAX as usize);
          for entry in palette {
            light.push(util::color(util::color_f32(&entry)));
            dark.push(util::color(util::inverted_color_f32(&entry)));
          }
          (light, dark)
        };

        // Wait for a message. Exit when the connection is closed.
        while let Ok(request) = trx.recv() {
          let mut part = request;

          // GDAL doesn't have any way to cancel a raster read operation and the
          // requests can pile up during a long read, so grab all the pending
          // requests in order to get to the most recent.
          while let Ok(request) = trx.try_recv() {
            part = request;
          }

          // Read the image data.
          match source.read(&part) {
            Ok(gdal_image) => {
              let ((w, h), data) = gdal_image.into_shape_and_vec();
              let mut px = Vec::with_capacity(w * h);

              // Choose the palette.
              let colors = if part.dark { &dark } else { &light };

              // Convert the palettized image to packed RGBA.
              for val in data {
                px.push(colors[val as usize]);
              }

              // Send the image data.
              let image = util::ImageData { w, h, px };
              ttx.send(RasterReply::Image(part, image)).unwrap();
            }
            Err(err) => {
              let text = format!("{err}");
              ttx.send(RasterReply::Error(part, text.into())).unwrap();
            }
          }
        }
      })
      .unwrap();

    Ok(Self {
      transformation,
      tx,
      rx,
    })
  }

  /// Get the transformation.
  pub fn transformation(&self) -> &Transformation {
    &self.transformation
  }

  /// Kick-off an image read operation.
  /// - `part`: the area to read from the source image.
  pub fn read_image(&self, part: ImagePart) {
    if part.rect.size.w > 0 && part.rect.size.h > 0 {
      self.tx.send(part).unwrap();
    }
  }

  /// Get the next available reply.
  pub fn get_reply(&self) -> Option<RasterReply> {
    self.rx.try_recv().ok()
  }
}

pub enum RasterReply {
  /// Image result from a read operation.
  Image(ImagePart, util::ImageData),

  /// Error message from a read operation.
  Error(ImagePart, util::Error),
}

/// Transformations between pixel, chart and decimal degree coordinates.
pub struct Transformation {
  // Full size of the chart raster in pixels.
  px_size: geom::Size,

  // The chart spatial reference.
  chart_sr: spatial_ref::SpatialRef,

  // Geo-transformation from chart coordinates to pixels.
  to_px: gdal::GeoTransform,

  // Geo-transformation from pixels to chart coordinates.
  from_px: gdal::GeoTransform,

  // Coordinate transformation from chart coordinates to decimal degrees.
  to_dd: spatial_ref::CoordTransform,

  // Coordinate transformation from decimal degrees to chart coordinates.
  from_dd: spatial_ref::CoordTransform,

  // Bounds in pixel coordinates.
  bounds: Vec<geom::Coord>,
}

impl Transformation {
  fn new(
    chart_name: &str,
    px_size: geom::Size,
    chart_sr: spatial_ref::SpatialRef,
    from_px: gdal::GeoTransform,
  ) -> Result<Self, gdal::errors::GdalError> {
    // FAA uses NAD83.
    let mut dd_sr = spatial_ref::SpatialRef::from_proj4(util::PROJ4_NAD83)?;

    // Respect X/Y order when converting to/from lat/lon coordinates.
    dd_sr.set_axis_mapping_strategy(spatial_ref::AxisMappingStrategy::TraditionalGisOrder);

    let to_dd = spatial_ref::CoordTransform::new(&chart_sr, &dd_sr)?;
    let from_dd = spatial_ref::CoordTransform::new(&dd_sr, &chart_sr)?;
    let to_px = gdal::GeoTransformEx::invert(&from_px)?;

    // Get the chart bounds.
    let bounds = config::get_chart_bounds(chart_name, px_size);

    Ok(Transformation {
      px_size,
      chart_sr,
      to_px,
      from_px,
      to_dd,
      from_dd,
      bounds,
    })
  }

  /// Get the spatial reference as a proj4 string.
  pub fn get_proj4(&self) -> String {
    self.chart_sr.to_proj4().unwrap()
  }

  /// The full size of the chart in pixels.
  pub fn px_size(&self) -> geom::Size {
    self.px_size
  }

  /// The bounds as pixel coordinates.
  pub fn pixel_bounds(&self) -> &Vec<geom::Coord> {
    &self.bounds
  }

  /// Get the bounds as chart coordinates.
  pub fn chart_bounds(&self) -> Vec<geom::Coord> {
    // Convert the pixel coordinates to chart coordinates.
    let mut chart_bounds = Vec::with_capacity(self.bounds.len());
    for point in &self.bounds {
      chart_bounds.push(self.px_to_chart(*point));
    }
    chart_bounds
  }

  /// Convert a pixel coordinate to a chart coordinate.
  /// - `coord`: pixel coordinate
  pub fn px_to_chart(&self, coord: geom::Coord) -> geom::Coord {
    gdal::GeoTransformEx::apply(&self.from_px, coord.x, coord.y).into()
  }

  /// Convert a chart coordinate to a pixel coordinate.
  /// - `coord`: chart coordinate
  pub fn chart_to_px(&self, coord: geom::Coord) -> geom::Coord {
    gdal::GeoTransformEx::apply(&self.to_px, coord.x, coord.y).into()
  }

  /// Convert a chart coordinate to a decimal degree coordinate.
  /// - `coord`: chart coordinate
  pub fn chart_to_dd(&self, coord: geom::Coord) -> Result<geom::Coord, gdal::errors::GdalError> {
    let mut x = [coord.x];
    let mut y = [coord.y];
    self.to_dd.transform_coords(&mut x, &mut y, &mut [])?;
    Ok(geom::Coord { x: x[0], y: y[0] })
  }

  /// Convert a decimal degree coordinate to a chart coordinate.
  /// - `coord`: decimal degree coordinate
  pub fn dd_to_chart(&self, coord: geom::Coord) -> Result<geom::Coord, gdal::errors::GdalError> {
    let mut x = [coord.x];
    let mut y = [coord.y];
    self.from_dd.transform_coords(&mut x, &mut y, &mut [])?;
    Ok(geom::Coord { x: x[0], y: y[0] })
  }

  #[allow(unused)]
  /// Convert a pixel coordinate to a decimal degree coordinate.
  /// - `coord`: pixel coordinate
  pub fn px_to_dd(&self, coord: geom::Coord) -> Result<geom::Coord, gdal::errors::GdalError> {
    self.chart_to_dd(self.px_to_chart(coord))
  }

  /// Convert a decimal degree coordinate to a pixel coordinate.
  /// - `coord`: decimal degree coordinate
  pub fn dd_to_px(&self, coord: geom::Coord) -> Result<geom::Coord, gdal::errors::GdalError> {
    Ok(self.chart_to_px(self.dd_to_chart(coord)?))
  }
}

/// The part of the image needed for display.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImagePart {
  pub rect: geom::Rect,
  pub zoom: util::Hashable,
  pub dark: bool,
}

impl ImagePart {
  pub fn new(rect: geom::Rect, zoom: f32, dark: bool) -> Self {
    // A zoom value of zero is not valid.
    assert!(zoom > 0.0);
    Self {
      rect,
      zoom: zoom.into(),
      dark,
    }
  }
}

/// Chart raster data source.
struct RasterSource {
  dataset: gdal::Dataset,
  band_idx: usize,
  px_size: geom::Size,
}

impl RasterSource {
  fn open_options<'a>() -> gdal::DatasetOptions<'a> {
    gdal::DatasetOptions {
      open_flags: gdal::GdalOpenFlags::GDAL_OF_READONLY
        | gdal::GdalOpenFlags::GDAL_OF_RASTER
        | gdal::GdalOpenFlags::GDAL_OF_INTERNAL,
      ..Default::default()
    }
  }

  /// Open a chart data source.
  /// - `path`: raster file path
  fn open(
    path: &path::Path,
  ) -> Result<(Self, Transformation, Vec<gdal::raster::RgbaEntry>), util::Error> {
    match gdal::Dataset::open_ex(path, Self::open_options()) {
      Ok(dataset) => {
        // Get and check the dataset's spatial reference.
        let spatial_ref = match dataset.spatial_ref() {
          Ok(sr) => {
            match sr.to_proj4() {
              Ok(proj4) => {
                let proj4 = proj4.to_lowercase();
                for item in ["+proj=lcc", "+datum=nad83", "+units=m"] {
                  if !proj4.contains(item) {
                    return Err("Unable to open chart:\ninvalid spatial reference".into());
                  }
                }
              }
              Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
            }
            sr
          }
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        };

        // This dataset must have a geo-transformation.
        let geo_transformation = match dataset.geo_transform() {
          Ok(gt) => gt,
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        };

        let px_size: geom::Size = dataset.raster_size().into();
        if !px_size.is_valid() {
          return Err("Unable to open chart:\ninvalid pixel size".into());
        }

        let chart_name = util::stem_str(path).unwrap();
        let transformation =
          match Transformation::new(chart_name, px_size, spatial_ref, geo_transformation) {
            Ok(trans) => trans,
            Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
          };

        let (band_idx, palette) = || -> Result<(usize, Vec<raster::RgbaEntry>), util::Error> {
          // The raster bands start at index one.
          for index in 1..=dataset.raster_count() {
            let rasterband = dataset.rasterband(index).unwrap();

            // The color interpretation for a FAA chart is PaletteIndex.
            if rasterband.color_interpretation() == raster::ColorInterpretation::PaletteIndex {
              if let Some(color_table) = rasterband.color_table() {
                // The color table must have 256 entries.
                let size = color_table.entry_count();
                if size != 256 {
                  return Err("Unable to open chart:\ninvalid color table".into());
                }

                // Collect the color entries as RGB.
                let mut palette = Vec::with_capacity(size);
                for index in 0..size {
                  if let Some(color) = color_table.entry_as_rgb(index) {
                    // All components must be in 0..256 range.
                    if util::check_color(color) {
                      palette.push(color);
                      continue;
                    }
                  }
                  return Err("Unable to open chart:\ninvalid color table".into());
                }
                return Ok((index, palette));
              }
              return Err("Unable to open chart:\ncolor table not found".into());
            }
          }
          Err("Unable to open chart:\nraster layer not found".into())
        }()?;

        Ok((
          Self {
            dataset,
            band_idx,
            px_size,
          },
          transformation,
          palette,
        ))
      }
      Err(err) => Err(format!("Unable to open chart:\n{err}").into()),
    }
  }

  fn read(&self, part: &ImagePart) -> Result<gdal::raster::Buffer<u8>, gdal::errors::GdalError> {
    // Scale and correct the source rectangle (GDAL does not tolerate
    // read requests outside the original raster size).
    let src_rect = part.rect.scaled(part.zoom.inverse()).fitted(self.px_size);
    let raster = self.dataset.rasterband(self.band_idx).unwrap();
    raster.read_as::<u8>(
      src_rect.pos.into(),
      src_rect.size.into(),
      part.rect.size.into(),
      Some(gdal::raster::ResampleAlg::Average),
    )
  }
}
