use crate::{chart, geom, util};
use godot::{
  classes::{
    image::Format, notify::ControlNotification, Control, IControl, Image, ImageTexture, InputEvent,
    InputEventMouseButton, InputEventMouseMotion, Texture2D,
  },
  global::{MouseButton, MouseButtonMask},
  prelude::*,
};
use std::path;

#[derive(GodotClass)]
#[class(base=Control)]
pub struct ChartWidget {
  base: Base<Control>,
  raster_reader: Option<chart::RasterReader>,
  chart_image: Option<ChartImage>,
  display_info: DisplayInfo,
  helicopter: bool,
}

impl ChartWidget {
  pub fn open_raster_reader(&mut self, path: &str, file: &str) -> Result<(), util::Error> {
    let file = path::Path::new(file);
    let helicopter = util::stem_str(file).unwrap().ends_with(" HEL");

    // Concatenate the VSI prefix and the file path.
    let path = ["/vsizip/", path].concat();
    let path = path::Path::new(path.as_str()).join(file);

    // Create a new raster reader.
    let raster_reader = chart::RasterReader::new(path)?;
    self.raster_reader = Some(raster_reader);
    self.display_info.origin = geom::Pos::default();
    self.display_info.zoom = 1.0;
    self.helicopter = helicopter;
    self.request_image();
    Ok(())
  }

  /// True if a helicopter chart is currently open.
  pub fn helicopter(&self) -> bool {
    self.helicopter
  }

  pub fn transformation(&self) -> Option<&chart::Transformation> {
    Some(self.raster_reader.as_ref()?.transformation())
  }

  pub fn set_night_mode(&mut self, dark: bool) {
    if self.display_info.dark != dark {
      self.display_info.dark = dark;
      self.request_image();
    }
  }

  pub fn goto_coord(&mut self, coord: geom::Coord) {
    let Some(raster_reader) = &self.raster_reader else {
      return;
    };

    match raster_reader.transformation().dd_to_px(coord) {
      Ok(px) => {
        let chart_size = raster_reader.transformation().px_size();
        if chart_size.contains(px) {
          let widget_size = self.base().get_size();
          let x = px.x as f32 - 0.5 * widget_size.x;
          let y = px.y as f32 - 0.5 * widget_size.y;

          self.display_info.zoom = 1.0;
          self.set_pos((x, y).into());
        }
      }
      Err(err) => godot_error!("{err}"),
    }
  }

  pub fn set_pos(&mut self, pos: geom::Pos) {
    let Some(pos) = self.correct_pos(pos) else {
      return;
    };

    if pos != self.display_info.origin {
      self.display_info.origin = pos;
      self.request_image();
      self.base_mut().queue_redraw();
    }
  }

  pub fn set_zoom(&mut self, zoom: f32, offset: Vector2) {
    let Some((zoom, pos)) = self.correct_zoom(zoom, offset) else {
      return;
    };

    if zoom != self.display_info.zoom || pos != self.display_info.origin {
      self.display_info.origin = pos;
      self.display_info.zoom = zoom;
      self.request_image();
      self.base_mut().queue_redraw();
    }
  }

  fn request_image(&self) {
    let Some(raster_reader) = &self.raster_reader else {
      return;
    };

    let pos = self.display_info.origin;
    let size = self.base().get_size().into();
    let rect = geom::Rect { pos, size };
    let part = chart::ImagePart::new(rect, self.display_info.zoom, self.display_info.dark);
    raster_reader.read_image(part);
  }

  fn get_chart_reply(&self) -> Option<ChartImage> {
    let raster_reader = self.raster_reader.as_ref()?;
    let mut image_info = None;

    // Collect all chart replies to get to the most recent image.
    while let Some(reply) = raster_reader.get_reply() {
      match reply {
        chart::RasterReply::Image(part, data) => {
          image_info = Some((part, data));
        }
        chart::RasterReply::Error(part, err) => {
          godot_error!("{err} @ {part:?}");
        }
      }
    }

    // Convert to texture.
    let (part, data) = image_info?;
    let texture = create_texture(data)?;
    Some(ChartImage { texture, part })
  }

  fn get_draw_info(&self) -> Option<(Gd<Texture2D>, Rect2)> {
    let chart_image = self.chart_image.as_ref()?;
    let zoom = self.display_info.zoom / chart_image.part.zoom.value();
    let pos = Vector2::from(chart_image.part.rect.pos) * zoom - self.display_info.origin.into();
    let size = chart_image.texture.get_size() * zoom;
    let rect = Rect2::new(pos, size);
    let texture = chart_image.texture.clone();
    Some((texture, rect))
  }

  fn get_chart_size(&self) -> Option<geom::Size> {
    let raster_reader = self.raster_reader.as_ref()?;
    Some(raster_reader.transformation().px_size())
  }

  fn correct_pos(&mut self, mut pos: geom::Pos) -> Option<geom::Pos> {
    let chart_size = self.get_chart_size()?;
    let max_size = chart_size * f64::from(self.display_info.zoom);
    let widget_size: geom::Size = self.base().get_size().into();

    // Make sure its within the horizontal limits.
    if pos.x < 0 {
      pos.x = 0;
    } else if pos.x + widget_size.w as i32 > max_size.w as i32 {
      pos.x = max_size.w as i32 - widget_size.w as i32;
    }

    // Make sure its within the vertical limits.
    if pos.y < 0 {
      pos.y = 0;
    } else if pos.y + widget_size.h as i32 > max_size.h as i32 {
      pos.y = max_size.h as i32 - widget_size.h as i32;
    }

    Some(pos)
  }

  fn correct_zoom(&mut self, zoom: f32, offset: Vector2) -> Option<(f32, geom::Pos)> {
    let chart_size = self.get_chart_size()?;

    // Clamp the zoom value.
    let mut zoom = zoom.clamp(ChartWidget::MIN_ZOOM, ChartWidget::MAX_ZOOM);

    let mut max_size = chart_size * f64::from(zoom);
    let widget_size: geom::Size = self.base().get_size().into();

    // Make sure the maximum chart size is not smaller than the widget.
    if max_size.w < widget_size.w {
      zoom = widget_size.w as f32 / chart_size.w as f32;
      max_size = chart_size * f64::from(zoom);
    }

    if max_size.h < widget_size.h {
      zoom = widget_size.h as f32 / chart_size.h as f32;
      max_size = chart_size * f64::from(zoom);
    }

    // Keep the zoom position at the offset.
    let pos = Vector2::from(self.display_info.origin) + offset;
    let pos = pos * zoom / self.display_info.zoom - offset;
    let mut pos = geom::Pos {
      x: pos.x.round() as i32,
      y: pos.y.round() as i32,
    };

    // Make sure its within the horizontal limits.
    if pos.x < 0 {
      pos.x = 0;
    } else if pos.x + widget_size.w as i32 > max_size.w as i32 {
      pos.x = max_size.w as i32 - widget_size.w as i32;
    }

    // Make sure its within the vertical limits.
    if pos.y < 0 {
      pos.y = 0;
    } else if pos.y + widget_size.h as i32 > max_size.h as i32 {
      pos.y = max_size.h as i32 - widget_size.h as i32;
    }

    Some((zoom, pos))
  }

  fn draw_bounds(&mut self) {
    let Some(raster_reader) = &self.raster_reader else {
      return;
    };

    // Get the chart bounds polygon in pixels.
    let source = raster_reader.transformation().points();
    if source.is_empty() {
      return;
    }

    // Scale and translate the coordinates to the current view.
    let zoom = self.display_info.zoom as f64;
    let pos = self.display_info.origin.into();
    let mut dest: Vec<Vector2> = Vec::with_capacity(source.len() + 1);
    for point in source {
      let point = *point * zoom - pos;
      dest.push(point.into());
    }

    // Close off the polygon.
    dest.push(*dest.first().unwrap());

    // Draw it as a polyline.
    let color = Color::from_rgb(1.0, 0.0, 1.0);
    let mut this = self.base_mut();
    this
      .draw_polyline_ex(&dest.into(), color)
      .width(1.0)
      .antialiased(true)
      .done();
  }

  const MIN_ZOOM: f32 = 1.0 / 8.0;
  const MAX_ZOOM: f32 = 1.0;
}

#[godot_api]
impl IControl for ChartWidget {
  fn init(base: Base<Control>) -> Self {
    Self {
      base,
      raster_reader: None,
      chart_image: None,
      display_info: DisplayInfo::new(),
      helicopter: false,
    }
  }

  fn on_notification(&mut self, what: ControlNotification) {
    if what == ControlNotification::RESIZED {
      let rect: geom::Rect = self.base().get_rect().into();
      if self.chart_image.is_some() {
        // Correct the current zoom (may change based on widget size).
        if let Some((zoom, _)) = self.correct_zoom(self.display_info.zoom, Vector2::default()) {
          self.display_info.zoom = zoom;

          let pos = if rect.pos.x == self.display_info.rect.pos.x {
            // Recenter the chart.
            self.display_info.origin + self.display_info.rect.center() - rect.center()
          } else {
            // Side panel was toggled, just compensate for that.
            self.display_info.origin + (rect.pos.x - self.display_info.rect.pos.x, 0).into()
          };

          // Correct the position.
          if let Some(pos) = self.correct_pos(pos) {
            self.display_info.origin = pos;
            self.request_image();
            self.base_mut().queue_redraw();
          }
        }
      }

      // Remember the widget rectangle for next time.
      self.display_info.rect = rect;
    }
  }

  fn draw(&mut self) {
    if let Some((texture, rect)) = self.get_draw_info() {
      self.base_mut().draw_texture_rect(&texture, rect, false);
    };

    self.draw_bounds();
  }

  fn process(&mut self, _delta: f64) {
    let chart_image = self.get_chart_reply();
    if chart_image.is_some() {
      self.chart_image = chart_image;
      self.base_mut().queue_redraw();
    }
  }

  fn gui_input(&mut self, event: Gd<InputEvent>) {
    if self.chart_image.is_none() {
      return;
    };

    if let Ok(event) = event.clone().try_cast::<InputEventMouseMotion>() {
      if event.get_button_mask() == MouseButtonMask::LEFT {
        let pos = self.display_info.origin - event.get_screen_relative().into();
        self.set_pos(pos);
      }
    } else if let Ok(event) = event.try_cast::<InputEventMouseButton>() {
      if event.is_pressed() {
        match event.get_button_index() {
          MouseButton::WHEEL_DOWN => {
            let zoom = self.display_info.zoom * 0.8;
            self.set_zoom(zoom, event.get_position());
          }
          MouseButton::WHEEL_UP => {
            let zoom = self.display_info.zoom * 1.25;
            self.set_zoom(zoom, event.get_position());
          }
          _ => (),
        };
      }
    }
  }
}

struct ChartImage {
  texture: Gd<Texture2D>,
  part: chart::ImagePart,
}

struct DisplayInfo {
  origin: geom::Pos,
  rect: geom::Rect,
  zoom: f32,
  dark: bool,
}

impl DisplayInfo {
  fn new() -> Self {
    Self {
      origin: geom::Pos::default(),
      rect: geom::Rect::default(),
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
  let image = Image::create_from_data(w, h, false, Format::RGBA8, &data)?;
  ImageTexture::create_from_image(&image).map(|texture| texture.upcast())
}
