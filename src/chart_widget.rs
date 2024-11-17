use crate::{chart, nasr, util};
use std::path;

use godot::{
  classes::{
    image::Format, notify::ControlNotification, Control, IControl, Image, ImageTexture, InputEvent,
    InputEventMouseMotion, Texture2D,
  },
  global::MouseButtonMask,
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Control)]
pub struct ChartWidget {
  base: Base<Control>,
  chart_reader: Option<chart::RasterReader>,
  airport_reader: Option<nasr::AirportReader>,
  chart_image: Option<ChartImage>,
  display_info: DisplayInfo,
}

impl ChartWidget {
  pub fn open_chart(&mut self, path: &str, file: &str) -> Result<(), util::Error> {
    // Concatenate the VSI prefix and the file path.
    let path = ["/vsizip/", path].concat();
    let path = path::Path::new(path.as_str()).join(file);

    // Create a new chart reader.
    match chart::RasterReader::new(path) {
      Ok(chart_reader) => {
        if let Some(airport_reader) = &self.airport_reader {
          // Send the chart spatial reference to the airport reader.
          let proj4 = chart_reader.transformation().get_proj4();
          let bounds = chart_reader.transformation().bounds().clone();
          airport_reader.set_spatial_ref(proj4, bounds);
        }

        self.chart_reader = Some(chart_reader);
        self.request_image();
        Ok(())
      }
      Err(err) => Err(err),
    }
  }

  pub fn open_airport_csv(&mut self, path: &str, file: &str) -> Result<(), util::Error> {
    // Concatenate the VSI prefix and the file path.
    let path = ["/vsizip//vsizip/", path].concat();
    let path = path::Path::new(path.as_str());
    let path = path.join(file).join("APT_BASE.csv");

    match nasr::AirportReader::new(path) {
      Ok(airport_reader) => {
        if let Some(chart_reader) = &self.chart_reader {
          // Send the chart spatial reference to the airport reader.
          let proj4 = chart_reader.transformation().get_proj4();
          let bounds = chart_reader.transformation().bounds().clone();
          airport_reader.set_spatial_ref(proj4, bounds);
        }

        self.airport_reader = Some(airport_reader);
        Ok(())
      }
      Err(err) => Err(err),
    }
  }

  pub fn airport_reader(&self) -> Option<&nasr::AirportReader> {
    self.airport_reader.as_ref()
  }

  pub fn set_night_mode(&mut self, dark: bool) {
    if self.display_info.dark != dark {
      self.display_info.dark = dark;
      self.request_image();
    }
  }

  fn request_image(&self) {
    if let Some(chart_reader) = &self.chart_reader {
      let pos = self.display_info.pos;
      let size = self.base().get_size().into();
      let rect = util::Rect { pos, size };
      let part = chart::ImagePart::new(rect, self.display_info.zoom, self.display_info.dark);

      // Check if the chart reader hash and the image part match.
      if let Some(chart_image) = &self.chart_image {
        if chart_image.hash == chart_reader.hash() && chart_image.part == part {
          return;
        }
      }

      chart_reader.read_image(part);
    }
  }

  fn get_chart_reply(&self) -> Option<ChartImage> {
    if let Some(chart_reader) = &self.chart_reader {
      let mut image_info = None;

      // Collect all chart replies to get to the most recent image.
      while let Some(reply) = chart_reader.get_reply() {
        match reply {
          chart::RasterReply::Image(part, data) => {
            image_info = Some((part, data));
          }
          chart::RasterReply::Error(part, err) => {
            godot_error!("{err} @ {part:?}");
          }
        }
      }

      // Convert to texture and return.
      if let Some((part, data)) = image_info {
        if let Some(texture) = create_texture(data) {
          return Some(ChartImage {
            texture,
            part,
            hash: chart_reader.hash(),
          });
        }
      }
    }

    None
  }

  fn get_draw_info(&self) -> Option<(Gd<Texture2D>, Rect2)> {
    if let Some(chart_image) = &self.chart_image {
      let pos = chart_image.part.rect.pos - self.display_info.pos;
      let size = chart_image.texture.get_size();
      let rect = Rect2::new(pos.into(), size);
      let texture = chart_image.texture.clone();
      return Some((texture, rect));
    }
    None
  }

  #[allow(unused)]
  const MIN_ZOOM: f32 = 1.0 / 8.0;
  #[allow(unused)]
  const MAX_ZOOM: f32 = 1.0;
}

#[godot_api]
impl IControl for ChartWidget {
  fn init(base: Base<Control>) -> Self {
    Self {
      base,
      chart_reader: None,
      airport_reader: None,
      chart_image: None,
      display_info: DisplayInfo::new(),
    }
  }

  fn on_notification(&mut self, what: ControlNotification) {
    if what == ControlNotification::RESIZED {
      self.request_image();
    }
  }

  fn draw(&mut self) {
    if let Some((texture, rect)) = self.get_draw_info() {
      self.base_mut().draw_texture_rect(&texture, rect, false);
    };
  }

  fn process(&mut self, _delta: f64) {
    let chart_image = self.get_chart_reply();
    if chart_image.is_some() {
      self.chart_image = chart_image;
      self.base_mut().queue_redraw();
    }
  }

  fn gui_input(&mut self, event: Gd<InputEvent>) {
    let Some(chart_image) = &self.chart_image else {
      return;
    };

    let Some(chart_reader) = &self.chart_reader else {
      return;
    };

    if let Ok(event) = event.try_cast::<InputEventMouseMotion>() {
      if event.get_button_mask() == MouseButtonMask::LEFT {
        // Modify the pan position.
        let mut pos = self.display_info.pos - event.get_screen_relative().into();

        let size = chart_image.part.rect.size;
        let max_size = chart_reader.transformation().px_size() * f64::from(chart_image.part.zoom);

        // Make sure its within the horizontal limits.
        if pos.x < 0 {
          pos.x = 0;
        } else if pos.x + size.w as i32 > max_size.w as i32 {
          pos.x = max_size.w as i32 - size.w as i32;
        }

        // Make sure its within the vertical limits.
        if pos.y < 0 {
          pos.y = 0;
        } else if pos.y + size.h as i32 > max_size.h as i32 {
          pos.y = max_size.h as i32 - size.h as i32;
        }

        if pos != self.display_info.pos {
          self.display_info.pos = pos;
          self.request_image();
          self.base_mut().queue_redraw();
        }
      }
    }
  }
}

struct ChartImage {
  texture: Gd<Texture2D>,
  part: chart::ImagePart,
  hash: u64,
}

struct DisplayInfo {
  pos: util::Pos,
  zoom: f32,
  dark: bool,
}

impl DisplayInfo {
  fn new() -> Self {
    Self {
      pos: util::Pos::default(),
      zoom: 1.0,
      dark: false,
    }
  }
}

/// Create a `Gd<Texture2D>` from `util::ImageData`.
fn create_texture(data: util::ImageData) -> Option<Gd<Texture2D>> {
  let w = data.w as i32;
  let h = data.h as i32;
  let data = data.px.as_flattened().into();
  if let Some(image) = Image::create_from_data(w, h, false, Format::RGBA8, &data) {
    return ImageTexture::create_from_image(&image).map(|texture| texture.upcast());
  }
  None
}
