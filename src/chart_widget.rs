use crate::{chart, util};
use chart::RasterReply;
use godot::{
  engine::{
    image::Format, notify::ControlNotification, Control, IControl, Image, ImageTexture, Texture2D,
  },
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Control)]
struct ChartWidget {
  base: Base<Control>,
  chart_source: Option<chart::RasterReader>,
  chart_image: Option<ChartImage>,
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

  fn ready(&mut self) {
    let path = "/vsizip//home/barugon/Downloads/FAA/Los_Angeles.zip/Los Angeles SEC.tif";
    self.chart_source = chart::RasterReader::new(path).ok();
  }

  fn on_notification(&mut self, what: ControlNotification) {
    if what == ControlNotification::RESIZED {
      if let Some(chart) = &self.chart_source {
        let this: Gd<Self> = self.to_gd();
        let rect = this.get_rect();
        let size = rect.size.into();
        let pos = (0, 0).into();
        let rect = util::Rect { pos, size };
        let part = chart::ImagePart::new(rect, 1.0, true);
        chart.read_image(part);
      }
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
          RasterReply::Image(part, image) => {
            if let Some(image) = Image::create_from_data(
              image.w as i32,
              image.h as i32,
              false,
              Format::RGBA8,
              image.px.into(),
            ) {
              if let Some(texture) = ImageTexture::create_from_image(image) {
                let mut this = self.to_gd();
                let texture: Gd<Texture2D> = texture.upcast();
                self.chart_image = Some(ChartImage { part, texture });
                this.queue_redraw();
              }
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
