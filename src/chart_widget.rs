use crate::{chart, util};
use chart::RasterReply;
use godot::{
  engine::{
    image::Format, notify::ControlNotification, Control, IControl, Image, ImageTexture, Texture2D,
  },
  prelude::*,
};
use std::path;

#[derive(GodotClass)]
#[class(base=Control)]
pub struct ChartWidget {
  base: Base<Control>,
  chart_source: Option<chart::RasterReader>,
  chart_image: Option<ChartImage>,
}

impl ChartWidget {
  pub fn open_chart(&mut self, path: &str, file: &str) -> Result<(), util::Error> {
    // Concatenate the VSI prefix and the file path.
    let path = ["/vsizip/", path].concat();
    let path = path::Path::new(path.as_str()).join(file);

    // Create a new chart reader.
    match chart::RasterReader::new(path) {
      Ok(chart_source) => {
        self.chart_source = Some(chart_source);
        self.request_image();
        Ok(())
      }
      Err(err) => Err(err),
    }
  }

  fn request_image(&self) {
    if let Some(chart_source) = &self.chart_source {
      let this = self.to_gd();
      let rect = this.get_rect();
      let size = rect.size.into();
      let pos = (0, 0).into();
      let part = chart::ImagePart::new(util::Rect { pos, size }, 1.0, true);
      chart_source.read_image(part);
    }
  }
}

#[godot_api]
impl IControl for ChartWidget {
  fn init(base: Base<Control>) -> Self {
    Self {
      base,
      chart_source: None,
      chart_image: None,
    }
  }

  fn on_notification(&mut self, what: ControlNotification) {
    if what == ControlNotification::RESIZED {
      self.request_image();
    }
  }

  fn draw(&mut self) {
    if let Some(chart_image) = &self.chart_image {
      let mut this = self.to_gd();
      let size = chart_image.texture.get_size();
      let rect = Rect2::from_components(0.0, 0.0, size.x, size.y);
      this.draw_texture_rect(chart_image.texture.clone(), rect, false);
    }
  }

  fn process(&mut self, _delta: f64) {
    // Collect any chart replies.
    if let Some(chart) = &self.chart_source {
      for reply in chart.get_replies() {
        match reply {
          RasterReply::Image(part, data) => {
            if let Some(texture) = create_texture(data) {
              let mut this = self.to_gd();
              self.chart_image = Some(ChartImage { part, texture });
              this.queue_redraw();
            }
          }
          RasterReply::Error(part, err) => {
            godot_error!("{err} @ {part:?}");
          }
        }
      }
    }
  }
}

struct ChartImage {
  #[allow(unused)]
  part: chart::ImagePart,
  texture: Gd<Texture2D>,
}

/// Create a `Gd<Texture2D>` from `util::ImageData`.
fn create_texture(data: util::ImageData) -> Option<Gd<Texture2D>> {
  let w = data.w as i32;
  let h = data.h as i32;
  let data = data.px.as_flattened().into();
  if let Some(image) = Image::create_from_data(w, h, false, Format::RGBA8, data) {
    return ImageTexture::create_from_image(image).map(|texture| texture.upcast());
  }
  None
}
