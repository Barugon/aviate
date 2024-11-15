use crate::{chart, util};
use godot::{
  classes::{
    image::Format, notify::ControlNotification, Control, IControl, Image, ImageTexture, Texture2D,
  },
  prelude::*,
};
use std::path;

#[derive(GodotClass)]
#[class(base=Control)]
pub struct ChartWidget {
  base: Base<Control>,
  chart_reader: Option<chart::RasterReader>,
  chart_image: Option<ChartImage>,
}

impl ChartWidget {
  pub fn open_chart(&mut self, path: &str, file: &str) -> Result<(), util::Error> {
    // Concatenate the VSI prefix and the file path.
    let path = ["/vsizip/", path].concat();
    let path = path::Path::new(path.as_str()).join(file);

    // Create a new chart reader.
    match chart::RasterReader::new(path) {
      Ok(chart_reader) => {
        self.chart_reader = Some(chart_reader);
        self.request_image();
        Ok(())
      }
      Err(err) => Err(err),
    }
  }

  fn request_image(&self) {
    if let Some(chart_reader) = &self.chart_reader {
      let size = self.base().get_size().into();
      let pos = (0, 0).into();
      let rect = util::Rect { pos, size };
      let part = chart::ImagePart::new(rect, 1.0, true);

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
      for reply in chart_reader.get_replies() {
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
      let size = chart_image.texture.get_size();
      let rect = Rect2::new(Vector2::new(0.0, 0.0), size);
      let texture = chart_image.texture.clone();
      return Some((texture, rect));
    }
    None
  }
}

#[godot_api]
impl IControl for ChartWidget {
  fn init(base: Base<Control>) -> Self {
    Self {
      base,
      chart_reader: None,
      chart_image: None,
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
}

struct ChartImage {
  texture: Gd<Texture2D>,
  part: chart::ImagePart,
  hash: u64,
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
