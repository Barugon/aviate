use crate::{config, geom, util};
use gdal::{errors, raster, spatial_ref};
use godot::{
  classes::{Image, ImageTexture, Texture2D, image::Format},
  obj::Gd,
};
use std::{any, array, cell, ops, path, sync::mpsc, thread};

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
    let (source, palette, transformation, chart_name) = Source::open(path)?;

    // Create the communication channels.
    let (sender, thread_receiver) = mpsc::channel::<Request>();
    let (thread_sender, receiver) = mpsc::channel::<Reply>();

    // Create the thread.
    thread::Builder::new()
      .name(any::type_name::<Reader>().to_owned())
      .spawn(move || {
        /// Convert a GDAL palette to light and dark ColorF32 palettes.
        fn convert_palette(palette: Vec<raster::RgbaEntry>) -> (PaletteF32, PaletteF32) {
          /// Convert a GDAL `RgbaEntry` to a `ColorF32`.
          fn color_f32(color: &raster::RgbaEntry) -> ColorF32 {
            // Convert colors to floating point in 0.0..=1.0 range.
            const SCALE: f32 = 1.0 / u8::MAX as f32;
            [
              color.r as f32 * SCALE,
              color.g as f32 * SCALE,
              color.b as f32 * SCALE,
              1.0,
            ]
          }

          /// Convert a GDAL `RgbaEntry` to a luminance inverted `ColorF32`.
          fn inverted_color_f32(color: &raster::RgbaEntry) -> ColorF32 {
            let [r, g, b, a] = color_f32(color);

            // Convert to YCbCr and invert the luminance.
            let y = 1.0 - (r * 0.299 + g * 0.587 + b * 0.114);
            let cb = b * 0.5 - r * 0.168736 - g * 0.331264;
            let cr = r * 0.5 - g * 0.418688 - b * 0.081312;

            // Convert back to RGB.
            let r = y + 1.402 * cr;
            let g = y - 0.344136 * cb - 0.714136 * cr;
            let b = y + 1.772 * cb;

            [r, g, b, a]
          }

          let palette = &palette[..PAL_LEN];
          let light = array::from_fn(|idx| color_f32(&palette[idx]));
          let dark = array::from_fn(|idx| inverted_color_f32(&palette[idx]));
          (light, dark)
        }

        /// Create a `Gd<Texture2D>` from `ImageData`.
        fn create_texture(data: Option<ImageData>) -> Option<Gd<Texture2D>> {
          let data = data?;
          let packed = data.px.into_flattened().into();
          let image = Image::create_from_data(data.w as i32, data.h as i32, false, Format::RGBA8, &packed)?;
          ImageTexture::create_from_image(&image).map(|texture| texture.upcast())
        }

        // Convert the color palette.
        let (light, dark) = convert_palette(palette);

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
              // Convert ImageData to a Gd<Texture2D> before sending.
              if let Some(texture) = create_texture(image) {
                let reply = Reply::Image(request.part, Texture(texture));
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
    let cancel = self.cancel_request();
    self.sender.send(Request { part, cancel }).unwrap();
  }

  /// Get the next available reply.
  pub fn get_reply(&self) -> Option<Reply> {
    self.receiver.try_recv().ok()
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

/// Wrapper for `Gd<Texture2D>` that implements `Send`.
pub struct Texture(Gd<Texture2D>);

// Should be improved by gdext v0.4 or v0.5 so that this unsafe wrapper is no longer necessary.
// https://github.com/godot-rust/gdext/issues/18
unsafe impl Send for Texture {}

impl From<Texture> for Gd<Texture2D> {
  fn from(texture: Texture) -> Self {
    texture.0
  }
}

pub enum Reply {
  /// Image result from a read operation.
  Image(ImagePart, Texture),

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
    for px in self.bounds.iter() {
      cht_poly.push(self.px_to_cht(*px));
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

#[derive(Clone, Copy, Debug)]
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
  fn open(path: &path::Path) -> Result<(Self, Vec<raster::RgbaEntry>, Transformation, String), util::Error> {
    macro_rules! error_msg {
      ($val:literal) => {
        util::Error::Borrowed(concat!("Unable to open chart:\n", $val))
      };
      ($val:expr) => {
        util::Error::Owned(format!("Unable to open chart:\n{}", $val))
      };
    }

    let dataset = match gdal::Dataset::open_ex(path, Self::open_options()) {
      Ok(dataset) => dataset,
      Err(err) => return Err(error_msg!(err)),
    };

    // Get the spatial reference from the dataset.
    let spatial_ref = match dataset.spatial_ref() {
      Ok(sr) => sr,
      Err(err) => return Err(error_msg!(err)),
    };

    // Check the spatial reference.
    match spatial_ref.to_proj4() {
      Ok(proj4) => {
        // A valid chart PROJ4 string must contain these terms.
        for item in ["+proj=lcc", "+datum=NAD83", "+units=m"] {
          if !proj4.contains(item) {
            return Err(error_msg!("invalid spatial reference"));
          }
        }
      }
      Err(err) => return Err(error_msg!(err)),
    }

    // Dataset must have a geo-transformation.
    let geo_trans = match dataset.geo_transform() {
      Ok(gt) => gt,
      Err(err) => return Err(error_msg!(err)),
    };

    let px_size: geom::Size = dataset.raster_size().into();
    if !px_size.is_valid() {
      return Err(error_msg!("invalid pixel size"));
    }

    let Some(chart_name) = util::stem_str(path) else {
      return Err(error_msg!("cannot determine chart name"));
    };

    let transformation = match Transformation::new(chart_name, px_size, spatial_ref, geo_trans) {
      Ok(trans) => trans,
      Err(err) => return Err(error_msg!(err)),
    };

    let (band_idx, palette) = || -> Result<(usize, Vec<raster::RgbaEntry>), util::Error> {
      /// Check if a GDAL `RgbaEntry` will fit into a `ColorU8`.
      fn check_color(color: &raster::RgbaEntry) -> bool {
        const MAX: i16 = u8::MAX as i16;
        const COMP_RANGE: ops::RangeInclusive<i16> = 0..=MAX;
        COMP_RANGE.contains(&color.r)
          && COMP_RANGE.contains(&color.g)
          && COMP_RANGE.contains(&color.b)
          && color.a == MAX
      }

      // Raster bands start at index one.
      for band_idx in 1..=dataset.raster_count() {
        let rasterband = match dataset.rasterband(band_idx) {
          Ok(rasterband) => rasterband,
          Err(err) => return Err(error_msg!(err)),
        };

        // The color interpretation for a FAA chart is PaletteIndex.
        if rasterband.color_interpretation() != raster::ColorInterpretation::PaletteIndex {
          continue;
        }

        let Some(color_table) = rasterband.color_table() else {
          return Err(error_msg!("color table not found"));
        };

        // The color table must have 256 entries.
        let size = color_table.entry_count();
        if size != PAL_LEN {
          return Err(error_msg!("invalid color table"));
        }

        // Collect the color entries as RGB.
        let mut palette = Vec::with_capacity(size);
        for index in 0..size {
          let Some(color) = color_table.entry_as_rgb(index) else {
            return Err(error_msg!("invalid color table"));
          };

          // All components must be in 0..256 range.
          if !check_color(&color) {
            return Err(error_msg!("color table contains invalid colors"));
          }

          palette.push(color);
        }

        return Ok((band_idx, palette));
      }
      Err(error_msg!("raster layer not found"))
    }()?;

    Ok((
      Self {
        dataset,
        band_idx,
        px_size,
      },
      palette,
      transformation,
      chart_name.into(),
    ))
  }

  /// Read and scale part of the chart raster image.
  fn read(&self, part: &ImagePart, pal: &PaletteF32, cancel: util::Cancel) -> errors::Result<Option<ImageData>> {
    if !part.is_valid() || cancel.canceled() {
      return Ok(None);
    }

    /// Structure that defines a raster area and provides some iterator-like functionality.
    struct Area {
      x: isize,
      y: isize,
      w: usize,
      h: usize,
      end: isize,
    }

    impl Area {
      fn new(rect: geom::Rect) -> Self {
        let x = rect.pos.x as isize;
        let y = rect.pos.y as isize;
        let w = rect.size.w as usize;
        let h = rect.size.h as usize;
        let end = y + h as isize;
        Self { x, y, w, h, end }
      }

      fn next(&mut self) -> bool {
        self.y += 1;
        self.y < self.end
      }

      fn pos(&self) -> (isize, isize) {
        (self.x, self.y)
      }
    }

    /// Convert a `ColorF32` to `ColorU8`
    fn color_u8(color: &ColorF32) -> ColorU8 {
      const SCALE: f32 = u8::MAX as f32;
      [
        (color[0] * SCALE) as u8,
        (color[1] * SCALE) as u8,
        (color[2] * SCALE) as u8,
        u8::MAX,
      ]
    }

    let raster = self.dataset.rasterband(self.band_idx)?;
    let mut src_area = Area::new(part.rect.scaled(1.0 / part.zoom).fitted(self.px_size));

    // Read the first source row.
    let src_buf = raster.read_as::<u8>(src_area.pos(), (src_area.w, 1), (src_area.w, 1), None)?;
    let (src_size, mut src_row) = src_buf.into_shape_and_vec();

    if part.zoom == 1.0 {
      let pal: [ColorU8; PAL_LEN] = array::from_fn(|idx| color_u8(&pal[idx]));
      let mut dst = Vec::with_capacity(src_area.w * src_area.h);

      loop {
        if cancel.canceled() {
          return Ok(None);
        }

        // Convert the pixels.
        for &idx in src_row.iter() {
          dst.push(pal[idx as usize]);
        }

        // Check for the end.
        if !src_area.next() {
          break;
        }

        // Read the next source row.
        raster.read_into_slice(src_area.pos(), src_size, src_size, &mut src_row, None)?;
      }

      return Ok(Some(ImageData {
        w: src_area.w,
        h: src_area.h,
        px: dst,
      }));
    }

    /// Process a source image row and accumulate into an intermediate result.
    fn process_row(dst: &mut [ColorF32], src: &[u8], pal: &PaletteF32, xr: f32, yr: f32) {
      /// Resample and accumulate a single pixel.
      fn resample(dst: &mut ColorF32, src: &ColorF32, scale: f32) {
        dst[0] += src[0] * scale;
        dst[1] += src[1] * scale;
        dst[2] += src[2] * scale;
        dst[3] += src[3] * scale;
      }

      let mut dst_iter = dst.iter_mut();
      let mut src_iter = src.iter();
      let mut ratio = xr;
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
        resample(dst, &pal[src as usize], ratio * yr);

        // Get the next source pixel.
        src = match src_iter.next() {
          Some(&src) => src,
          None => break,
        };

        remain -= ratio;
        ratio = xr;
        if remain < xr {
          if remain > 0.0 {
            // Resample what remains of this pixel.
            resample(dst, &pal[src as usize], remain * yr);
          }

          // Move to the next destination pixel.
          dst = match dst_iter.next() {
            Some(dst) => dst,
            None => break,
          };

          ratio = xr - remain;
          remain = 1.0;
        }
      }
    }

    let mut dst_area = Area::new(part.rect);
    let mut acc_row = vec![ColorF32::default(); dst_area.w];
    let mut dst = Vec::with_capacity(dst_area.w * dst_area.h);
    let mut ratio = part.zoom;
    let mut remain = 1.0;

    loop {
      if cancel.canceled() {
        return Ok(None);
      }

      // Process the source row.
      process_row(&mut acc_row, &src_row, pal, part.zoom, ratio);

      // Check if the end of the source data has been reached.
      if !src_area.next() {
        // Output this row.
        for acc_px in acc_row.iter() {
          dst.push(color_u8(acc_px));
        }
        break;
      }

      // Read the next source row.
      raster.read_into_slice(src_area.pos(), src_size, src_size, &mut src_row, None)?;

      remain -= ratio;
      ratio = part.zoom;
      if remain < part.zoom {
        if remain > 0.0 {
          // Process the remaining amount from this source row.
          process_row(&mut acc_row, &src_row, pal, part.zoom, remain);
        }

        // Output the row.
        for acc_px in acc_row.iter_mut() {
          dst.push(color_u8(acc_px));
          *acc_px = ColorF32::default();
        }

        // Check if the end of the destination has been reached.
        if !dst_area.next() {
          break;
        }

        ratio = part.zoom - remain;
        remain = 1.0;
      }
    }

    Ok(Some(ImageData {
      w: dst_area.w,
      h: dst_area.h,
      px: dst,
    }))
  }
}

struct ImageData {
  w: usize,
  h: usize,
  px: Vec<ColorU8>,
}

type ColorF32 = [f32; 4];
type ColorU8 = [u8; 4];

const PAL_LEN: usize = u8::MAX as usize + 1;
type PaletteF32 = [ColorF32; PAL_LEN];
