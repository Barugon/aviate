use crate::{config, geom, util};
use gdal::{errors, raster, spatial_ref};
use std::{any, array, cell, path, sync::mpsc, thread};

/// Reader is used for opening and reading
/// [VFR charts](https://www.faa.gov/air_traffic/flight_info/aeronav/digital_products/vfr/) in zipped GEO-TIFF format.
pub struct Reader {
  chart_name: String,
  transformation: Transformation,
  sender: mpsc::Sender<Request>,
  receiver: mpsc::Receiver<Reply>,
  cancel: cell::Cell<Option<util::Cancel>>,
}

impl Reader {
  /// Create a new chart reader.
  /// - `path`: chart file path
  pub fn new(path: &path::Path) -> Result<Self, util::Error> {
    // Open the chart source.
    let (source, transformation, palette) = Source::open(path)?;
    let chart_name = util::stem_str(path).unwrap().into();

    // Create the communication channels.
    let (sender, thread_receiver) = mpsc::channel::<Request>();
    let (thread_sender, receiver) = mpsc::channel::<Reply>();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<Reader>().to_owned())
      .spawn(move || {
        // Convert the color palette.
        assert!(palette.len() == PAL_LEN);
        let light: PaletteF32 = array::from_fn(|idx| util::color_f32(&palette[idx]));
        let dark: PaletteF32 = array::from_fn(|idx| util::inverted_color_f32(&palette[idx]));
        drop(palette);

        // Wait for a message. Exit when the connection is closed.
        while let Ok(request) = thread_receiver.recv() {
          // Choose the palette.
          let pal = match request.part.pal_type {
            PaletteType::Light => &light,
            PaletteType::Dark => &dark,
          };

          // Read the image data.
          match source.read(&request.part, pal, request.cancel) {
            Ok(image) => {
              if let Some(image) = image {
                let reply = Reply::Image(request.part, image);
                thread_sender.send(reply).unwrap();
              }
            }
            Err(err) => {
              let reply = Reply::Error(request.part, format!("{err}").into());
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
    self.cancel_request();

    let cancel = util::Cancel::default();
    self.cancel.replace(Some(cancel.clone()));
    self.sender.send(Request { part, cancel }).unwrap();
  }

  /// Get the next available reply.
  pub fn get_reply(&self) -> Option<Reply> {
    self.receiver.try_recv().ok()
  }

  fn cancel_request(&self) {
    if let Some(mut cancel) = self.cancel.take() {
      cancel.cancel();
    }
  }
}

impl Drop for Reader {
  fn drop(&mut self) {
    self.cancel_request();
  }
}

pub enum Reply {
  /// Image result from a read operation.
  Image(ImagePart, util::ImageData),

  /// Error message from a read operation.
  Error(ImagePart, util::Error),
}

/// Transformations between pixel, chart and decimal-degree coordinates.
pub struct Transformation {
  // Full size of the chart in pixels.
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
  bounds: Vec<geom::Px>,
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
  pub fn pixel_bounds(&self) -> &Vec<geom::Px> {
    &self.bounds
  }

  /// Get the bounds as chart coordinates.
  pub fn get_chart_bounds(&self) -> geom::Bounds {
    // Convert the pixel coordinates to chart coordinates.
    let mut cht_poly = Vec::with_capacity(self.bounds.len());
    for point in self.bounds.iter() {
      cht_poly.push(self.px_to_cht(*point));
    }
    geom::Bounds::new(cht_poly)
  }

  /// Convert a pixel coordinate to a chart coordinate.
  /// - `coord`: pixel coordinate
  pub fn px_to_cht(&self, coord: geom::Px) -> geom::Cht {
    let (x, y) = gdal::GeoTransformEx::apply(&self.from_px, coord.x, coord.y);
    geom::Cht::new(x, y)
  }

  /// Convert a chart coordinate to a pixel coordinate.
  /// - `coord`: chart coordinate
  pub fn cht_to_px(&self, coord: geom::Cht) -> geom::Px {
    let (x, y) = gdal::GeoTransformEx::apply(&self.to_px, coord.x, coord.y);
    geom::Px::new(x, y)
  }

  /// Convert a chart coordinate to a decimal-degree coordinate.
  /// - `coord`: chart coordinate
  pub fn cht_to_dd(&self, coord: geom::Cht) -> errors::Result<geom::DD> {
    use geom::Transform;
    Ok(self.to_dd.transform(*coord)?.into())
  }

  /// Convert a decimal-degree coordinate to a chart coordinate.
  /// - `coord`: decimal-degree coordinate
  pub fn dd_to_cht(&self, coord: geom::DD) -> errors::Result<geom::Cht> {
    use geom::Transform;
    Ok(self.from_dd.transform(*coord)?.into())
  }

  /// Convert a pixel coordinate to a decimal-degree coordinate.
  /// - `coord`: pixel coordinate
  #[allow(unused)]
  pub fn px_to_dd(&self, coord: geom::Px) -> errors::Result<geom::DD> {
    self.cht_to_dd(self.px_to_cht(coord))
  }

  /// Convert a decimal-degree coordinate to a pixel coordinate.
  /// - `coord`: decimal-degree coordinate
  pub fn dd_to_px(&self, coord: geom::DD) -> errors::Result<geom::Px> {
    Ok(self.cht_to_px(self.dd_to_cht(coord)?))
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

const PAL_LEN: usize = u8::MAX as usize + 1;
type PaletteF32 = [util::ColorF32; PAL_LEN];
type PaletteU8 = [util::ColorU8; PAL_LEN];

struct Request {
  part: ImagePart,
  cancel: util::Cancel,
}

/// Chart data source.
struct Source {
  dataset: gdal::Dataset,
  band_idx: usize,
  px_size: geom::Size,
}

impl Source {
  fn open_options<'a>() -> gdal::DatasetOptions<'a> {
    gdal::DatasetOptions {
      open_flags: gdal::GdalOpenFlags::GDAL_OF_READONLY
        | gdal::GdalOpenFlags::GDAL_OF_RASTER
        | gdal::GdalOpenFlags::GDAL_OF_INTERNAL,
      ..Default::default()
    }
  }

  /// Open a chart data source.
  /// - `path`: chart file path
  fn open(path: &path::Path) -> Result<(Self, Transformation, Vec<gdal::raster::RgbaEntry>), util::Error> {
    match gdal::Dataset::open_ex(path, Self::open_options()) {
      Ok(dataset) => {
        // Get the spatial reference from the dataset.
        let spatial_ref = match dataset.spatial_ref() {
          Ok(sr) => sr,
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        };

        // Check the spatial reference.
        match spatial_ref.to_proj4() {
          Ok(proj4) => {
            // A valid chart PROJ4 string must contain these terms.
            for item in ["+proj=lcc", "+datum=NAD83", "+units=m"] {
              if !proj4.contains(item) {
                return Err("Unable to open chart:\ninvalid spatial reference".into());
              }
            }
          }
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        }

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
        let transformation = match Transformation::new(cht_name, px_size, spatial_ref, geo_trans) {
          Ok(trans) => trans,
          Err(err) => return Err(format!("Unable to open chart:\n{err}").into()),
        };

        let (band_idx, palette) = || -> Result<(usize, Vec<raster::RgbaEntry>), util::Error> {
          // Raster bands start at index one.
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
            if size != PAL_LEN {
              return Err("Unable to open chart:\ninvalid color table".into());
            }

            // Collect the color entries as RGB.
            let mut palette = Vec::with_capacity(size);
            for index in 0..size {
              let Some(color) = color_table.entry_as_rgb(index) else {
                return Err("Unable to open chart:\ninvalid color table".into());
              };

              // All components must be in 0..256 range.
              if !util::check_color(&color) {
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

  fn read(&self, part: &ImagePart, pal: &PaletteF32, cancel: util::Cancel) -> errors::Result<Option<util::ImageData>> {
    if !part.is_valid() || cancel.canceled() {
      return Ok(None);
    }

    let raster = self.dataset.rasterband(self.band_idx).unwrap();
    let src_rect = part.rect.scaled(1.0 / part.zoom).fitted(self.px_size);
    let src_end = src_rect.pos.y as isize + src_rect.size.h as isize;
    let sw = src_rect.size.w as usize;
    let sh = src_rect.size.h as usize;
    let sx = src_rect.pos.x as isize;
    let mut sy = src_rect.pos.y as isize;

    // Read the first source row.
    let src_buf = raster.read_as::<u8>((sx, sy), (sw, 1), (sw, 1), None)?;
    let (_, mut src_row) = src_buf.into_shape_and_vec();

    if part.zoom == 1.0 {
      let pal: PaletteU8 = array::from_fn(|idx| util::color_u8(&pal[idx]));
      let mut dst = Vec::with_capacity(sw * sh);

      loop {
        if cancel.canceled() {
          return Ok(None);
        }

        // Convert the pixels.
        for &idx in &src_row {
          dst.push(pal[idx as usize]);
        }

        // Check for the end.
        sy += 1;
        if sy == src_end {
          break;
        }

        // Read the next source row.
        raster.read_into_slice((sx, sy), (sw, 1), (sw, 1), &mut src_row, None)?;
      }

      return Ok(Some(util::ImageData { w: sw, h: sh, px: dst }));
    }

    /// Process a source image row and accumulate into an intermediate result.
    fn process_row(dst: &mut [util::ColorF32], src: &[u8], pal: &PaletteF32, xr: f32, yr: f32) {
      let mut dst_iter = dst.iter_mut();
      let mut src_iter = src.iter();
      let mut portion = xr;
      let mut remain = 1.0;

      let mut dst = match dst_iter.next() {
        Some(dst) => dst,
        None => return,
      };

      let mut src = match src_iter.next() {
        Some(&src) => src,
        None => return,
      };

      loop {
        // Resample the source pixel.
        let rgb = &pal[src as usize];
        let ratio = portion * yr;
        dst[0] += rgb[0] * ratio;
        dst[1] += rgb[1] * ratio;
        dst[2] += rgb[2] * ratio;

        // Get the next source pixel.
        src = match src_iter.next() {
          Some(&src) => src,
          None => break,
        };

        remain -= portion;
        portion = xr;

        if remain < xr {
          if remain > 0.0 {
            // Resample what remains of this pixel.
            let rgb = &pal[src as usize];
            let ratio = remain * yr;
            dst[0] += rgb[0] * ratio;
            dst[1] += rgb[1] * ratio;
            dst[2] += rgb[2] * ratio;
          }

          // Move to the next destination pixel.
          dst = match dst_iter.next() {
            Some(dst) => dst,
            None => break,
          };

          portion = xr - remain;
          remain = 1.0;
        }
      }
    }

    let dw = part.rect.size.w as usize;
    let dh = part.rect.size.h as usize;
    let mut dy = 0;
    let mut int_row = vec![[0.0, 0.0, 0.0]; dw];
    let mut dst = Vec::with_capacity(dw * dh);
    let mut portion = part.zoom;
    let mut remain = 1.0;

    loop {
      // Check if the operation has been canceled.
      if cancel.canceled() {
        return Ok(None);
      }

      // Process the source row.
      process_row(&mut int_row, &src_row, pal, part.zoom, portion);

      // Check if the end of the source data has been reached.
      sy += 1;
      if sy == src_end {
        // Output this row if the end of the destination data hasn't been reached.
        if dy < dh {
          for int_px in &int_row {
            dst.push(util::color_u8(int_px));
          }
        }
        break;
      }

      // Read the next source row.
      raster.read_into_slice((sx, sy), (sw, 1), (sw, 1), &mut src_row, None)?;

      remain -= portion;
      portion = part.zoom;
      if remain < part.zoom {
        if remain > 0.0 {
          // Process the final amount from this source row.
          process_row(&mut int_row, &src_row, pal, part.zoom, remain);
        }

        // Output the row.
        for int_px in &mut int_row {
          dst.push(util::color_u8(int_px));
          *int_px = [0.0, 0.0, 0.0];
        }

        // Check if the end of the destination data has been reached.
        dy += 1;
        if dy == dh {
          break;
        }

        portion = part.zoom - remain;
        remain = 1.0;
      }
    }

    Ok(Some(util::ImageData { w: dw, h: dh, px: dst }))
  }
}
