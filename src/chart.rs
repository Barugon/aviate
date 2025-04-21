use crate::{config, geom, util};
use gdal::{
  errors, raster,
  spatial_ref::{self, SpatialRef},
};
use std::{any, cell, path, sync::mpsc, thread};

/// RasterReader is used for opening and reading
/// [VFR charts](https://www.faa.gov/air_traffic/flight_info/aeronav/digital_products/vfr/) in zipped GEO-TIFF format.
pub struct RasterReader {
  chart_name: String,
  transformation: Transformation,
  sender: mpsc::Sender<RasterRequest>,
  receiver: mpsc::Receiver<RasterReply>,
  cancel: cell::Cell<Option<util::Cancel>>,
}

impl RasterReader {
  /// Create a new chart raster reader.
  /// - `path`: chart file path
  pub fn new(path: &path::Path) -> Result<Self, util::Error> {
    // Open the chart source.
    let (source, transformation, palette) = RasterSource::open(path)?;
    let chart_name = util::stem_str(path).unwrap().into();

    // Create the communication channels.
    let (sender, thread_receiver) = mpsc::channel::<RasterRequest>();
    let (thread_sender, receiver) = mpsc::channel::<RasterReply>();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<RasterReader>().to_owned())
      .spawn(move || {
        let (light, dark) = {
          // Convert the color palette.
          let mut light = Vec::with_capacity(u8::MAX as usize);
          let mut dark = Vec::with_capacity(u8::MAX as usize);
          for entry in palette {
            light.push(util::color_f32(&entry));
            dark.push(util::inverted_color_f32(&entry));
          }
          (light, dark)
        };

        // Wait for a message. Exit when the connection is closed.
        while let Ok(request) = thread_receiver.recv() {
          let request = {
            let mut request = request;

            // Get to the most recent request.
            while let Ok(try_request) = thread_receiver.try_recv() {
              request = try_request;
            }
            request
          };

          // Choose the palette.
          let pal = match request.part.pal_type {
            PaletteType::Light => &light,
            PaletteType::Dark => &dark,
          };

          // Read the image data.
          match source.read(&request.part, pal, request.cancel) {
            Ok(image) => {
              if let Some(image) = image {
                let reply = RasterReply::Image(request.part, image);
                thread_sender.send(reply).unwrap();
              }
            }
            Err(err) => {
              let reply = RasterReply::Error(request.part, format!("{err}").into());
              thread_sender.send(reply).unwrap();
            }
          }
        }
      })
      .unwrap();

    Ok(Self {
      chart_name,
      transformation,
      sender,
      receiver,
      cancel: cell::Cell::new(None),
    })
  }

  /// Get the chart name.
  pub fn chart_name(&self) -> &str {
    &self.chart_name
  }

  /// Get the transformation.
  pub fn transformation(&self) -> &Transformation {
    &self.transformation
  }

  /// Kick-off an image read operation.
  /// - `part`: the area to read from the source image.
  pub fn read_image(&self, part: ImagePart) {
    assert!(part.is_valid());
    self.cancel_read();

    let cancel = util::Cancel::default();
    self.cancel.replace(Some(cancel.clone()));
    self.sender.send(RasterRequest { part, cancel }).unwrap();
  }

  /// Get the next available reply.
  pub fn get_reply(&self) -> Option<RasterReply> {
    self.receiver.try_recv().ok()
  }

  fn cancel_read(&self) {
    if let Some(mut cancel) = self.cancel.take() {
      cancel.cancel();
    }
  }
}

impl Drop for RasterReader {
  fn drop(&mut self) {
    self.cancel_read();
  }
}

pub enum RasterReply {
  /// Image result from a read operation.
  Image(ImagePart, util::ImageData),

  /// Error message from a read operation.
  Error(ImagePart, util::Error),
}

/// Transformations between pixel, chart and decimal-degree coordinates.
pub struct Transformation {
  // Full size of the chart raster in pixels.
  px_size: geom::Size,

  // The chart spatial reference.
  chart_sr: spatial_ref::SpatialRef,

  // Geo-transformation from chart coordinates to pixels.
  to_px: gdal::GeoTransform,

  // Geo-transformation from pixels to chart coordinates.
  from_px: gdal::GeoTransform,

  // Coordinate transformation from chart coordinates to decimal-degree coordinates.
  to_dd: spatial_ref::CoordTransform,

  // Coordinate transformation from decimal-degree coordinates to chart coordinates.
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
  ) -> errors::Result<Self> {
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
  pub fn get_chart_bounds(&self) -> geom::Bounds {
    // Convert the pixel coordinates to chart coordinates.
    let mut cht_poly = Vec::with_capacity(self.bounds.len());
    for point in self.bounds.iter() {
      cht_poly.push(self.px_to_chart(*point));
    }
    geom::Bounds::new(cht_poly)
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

  /// Convert a chart coordinate to a decimal-degree coordinate.
  /// - `coord`: chart coordinate
  pub fn chart_to_dd(&self, coord: geom::Coord) -> errors::Result<geom::Coord> {
    use geom::Transform;
    self.to_dd.transform(coord)
  }

  /// Convert a decimal-degree coordinate to a chart coordinate.
  /// - `coord`: decimal-degree coordinate
  pub fn dd_to_chart(&self, coord: geom::Coord) -> errors::Result<geom::Coord> {
    use geom::Transform;
    self.from_dd.transform(coord)
  }

  /// Convert a pixel coordinate to a decimal-degree coordinate.
  /// - `coord`: pixel coordinate
  #[allow(unused)]
  pub fn px_to_dd(&self, coord: geom::Coord) -> errors::Result<geom::Coord> {
    self.chart_to_dd(self.px_to_chart(coord))
  }

  /// Convert a decimal-degree coordinate to a pixel coordinate.
  /// - `coord`: decimal-degree coordinate
  pub fn dd_to_px(&self, coord: geom::Coord) -> errors::Result<geom::Coord> {
    Ok(self.chart_to_px(self.dd_to_chart(coord)?))
  }
}

#[derive(Clone, Debug)]
pub enum PaletteType {
  Light,
  Dark,
}

/// The part of the image needed for display.
#[derive(Clone, Debug)]
pub struct ImagePart {
  pub rect: geom::Rect,
  pub zoom: f32,
  pub pal_type: PaletteType,
}

impl ImagePart {
  pub fn new(rect: geom::Rect, zoom: f32, pal_type: PaletteType) -> Self {
    Self { rect, zoom, pal_type }
  }

  pub fn is_valid(&self) -> bool {
    self.rect.size.is_valid() && util::ZOOM_RANGE.contains(&self.zoom)
  }
}

struct RasterRequest {
  part: ImagePart,
  cancel: util::Cancel,
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

  fn check_spatial_ref(sr: SpatialRef) -> Result<SpatialRef, util::Error> {
    match sr.to_proj4() {
      Ok(proj4) => {
        for item in ["+proj=lcc", "+datum=NAD83", "+units=m"] {
          if !proj4.contains(item) {
            return Err("Unable to open chart:\ninvalid spatial reference".into());
          }
        }
        Ok(sr)
      }
      Err(err) => Err(format!("Unable to open chart:\n{err}").into()),
    }
  }

  /// Open a chart data source.
  /// - `path`: raster file path
  fn open(path: &path::Path) -> Result<(Self, Transformation, Vec<gdal::raster::RgbaEntry>), util::Error> {
    match gdal::Dataset::open_ex(path, Self::open_options()) {
      Ok(dataset) => {
        // Get and check the dataset's spatial reference.
        let cht_sr = match dataset.spatial_ref() {
          Ok(sr) => Self::check_spatial_ref(sr)?,
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        };

        // Dataset must have a geo-transformation.
        let geo_trans = match dataset.geo_transform() {
          Ok(gt) => gt,
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        };

        let px_size: geom::Size = dataset.raster_size().into();
        if !px_size.is_valid() {
          return Err("Unable to open chart:\ninvalid pixel size".into());
        }

        let cht_name = util::stem_str(path).unwrap();
        let transformation = match Transformation::new(cht_name, px_size, cht_sr, geo_trans) {
          Ok(trans) => trans,
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        };

        let (band_idx, palette) = || -> Result<(usize, Vec<raster::RgbaEntry>), util::Error> {
          // The raster bands start at index one.
          for index in 1..=dataset.raster_count() {
            let rasterband = dataset.rasterband(index).unwrap();

            // The color interpretation for a FAA chart is PaletteIndex.
            if rasterband.color_interpretation() != raster::ColorInterpretation::PaletteIndex {
              continue;
            }

            let Some(color_table) = rasterband.color_table() else {
              return Err("Unable to open chart:\ncolor table not found".into());
            };

            // The color table must have 256 entries.
            let size = color_table.entry_count();
            if size != 256 {
              return Err("Unable to open chart:\ninvalid color table".into());
            }

            // Collect the color entries as RGB.
            let mut palette = Vec::with_capacity(size);
            for index in 0..size {
              let Some(color) = color_table.entry_as_rgb(index) else {
                return Err("Unable to open chart:\ninvalid color table".into());
              };

              // All components must be in 0..256 range.
              if !util::check_color(color) {
                return Err("Unable to open chart:\ncolor table contains invalid colors".into());
              }

              palette.push(color);
            }

            return Ok((index, palette));
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

  fn read(&self, part: &ImagePart, pal: &[[f32; 3]], cancel: util::Cancel) -> errors::Result<Option<util::ImageData>> {
    if !part.is_valid() {
      return Ok(None);
    }

    /// Process a source image row and accumulate into an intermediate result.
    fn process_row(dst: &mut [[f32; 3]], src: &[u8], pal: &[[f32; 3]], xr: f32, yr: f32) {
      let mut dst_iter = dst.iter_mut();
      let mut src_iter = src.iter();
      let mut portion = xr;
      let mut remain = 1.0;

      let Some(mut dst) = dst_iter.next() else {
        return;
      };

      let Some(mut src) = src_iter.next() else {
        return;
      };

      loop {
        // Resample the source pixel.
        let rgb = &pal[*src as usize];
        let ratio = portion * yr;
        dst[0] += rgb[0] * ratio;
        dst[1] += rgb[1] * ratio;
        dst[2] += rgb[2] * ratio;

        // Move to the next source pixel.
        let Some(src_next) = src_iter.next() else {
          break;
        };

        src = src_next;
        remain -= portion;
        portion = xr;

        if remain < xr {
          // Resample what remains of this pixel.
          let rgb = &pal[*src as usize];
          let ratio = remain * yr;
          dst[0] += rgb[0] * ratio;
          dst[1] += rgb[1] * ratio;
          dst[2] += rgb[2] * ratio;

          // Move to the next destination pixel.
          let Some(dst_next) = dst_iter.next() else {
            break;
          };

          dst = dst_next;
          portion = xr - remain;
          remain = 1.0;
        }
      }
    }

    let raster = self.dataset.rasterband(self.band_idx).unwrap();
    let scale = part.zoom;
    let dst_rect = part.rect;
    let src_rect = dst_rect.scaled(1.0 / scale).fitted(self.px_size);
    let sw = src_rect.size.w as usize;
    let sh = src_rect.size.h as usize;
    let sx = src_rect.pos.x as isize;
    let src_end = src_rect.pos.y as isize + sh as isize;
    let dw = dst_rect.size.w as usize;
    let dh = dst_rect.size.h as usize;
    let mut int_row = vec![[0.0, 0.0, 0.0]; dw];
    let mut dst = Vec::with_capacity(dw * dh);
    let mut portion = scale;
    let mut remain = 1.0;
    let mut sy = src_rect.pos.y as isize;
    let mut dy = 0;

    // Read the first source row.
    let src_buf = raster.read_as::<u8>((sx, sy), (sw, 1), (sw, 1), None)?;
    let (_, mut src_row) = src_buf.into_shape_and_vec();

    loop {
      // Check if the operation has been canceled.
      if cancel.canceled() {
        return Ok(None);
      }

      // Process the source row.
      process_row(&mut int_row, &src_row, pal, scale, portion);

      // Check if the end of the source data has been reached.
      sy += 1;
      if sy == src_end {
        // Output this row if the end of the destination data hasn't been reached.
        if dy < dh {
          for rgb in &int_row {
            dst.push(util::color(*rgb));
          }
        }
        break;
      }

      // Read the next source row.
      raster.read_into_slice((sx, sy), (sw, 1), (sw, 1), &mut src_row, None)?;

      remain -= portion;
      portion = scale;
      if remain < scale {
        // Process the final amount from this source row.
        process_row(&mut int_row, &src_row, pal, scale, remain);

        // Output the destination row.
        for rgb in &mut int_row {
          dst.push(util::color(*rgb));
          *rgb = [0.0, 0.0, 0.0];
        }

        // Check if the end of the destination data has been reached.
        dy += 1;
        if dy == dh {
          break;
        }

        portion = scale - remain;
        remain = 1.0;
      }
    }

    Ok(Some(util::ImageData { w: dw, h: dh, px: dst }))
  }
}
