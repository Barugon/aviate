use crate::{chart, geom, util};
use godot::{
  classes::{
    Control, IControl, InputEvent, InputEventMagnifyGesture, InputEventMouseButton, InputEventMouseMotion,
    InputEventScreenTouch, Texture2D, notify::ControlNotification,
  },
  global::{MouseButton, MouseButtonMask},
  prelude::*,
};
use std::path;

#[derive(GodotClass)]
#[class(base=Control)]
pub struct ChartWidget {
  base: Base<Control>,
  raster_reader: Option<chart::Reader>,
  chart_image: Option<ChartImage>,
  display_info: DisplayInfo,
  heliport: bool,
}

impl ChartWidget {
  pub fn open_raster_reader(&mut self, path: &str, file: &str) -> Result<(), util::Error> {
    // Concatenate the VSI prefix and the file path.
    let path = ["/vsizip/", path].concat();
    let path = path::Path::new(path.as_str()).join(file);

    // Create a new raster reader.
    let raster_reader = chart::Reader::new(&path)?;
    self.heliport = raster_reader.chart_name().ends_with(" HEL");
    self.raster_reader = Some(raster_reader);
    self.display_info.origin = geom::Pos::default();
    self.display_info.zoom = 1.0;
    self.request_image();
    Ok(())
  }

  pub fn chart_name(&self) -> Option<&str> {
    let raster_reader = self.raster_reader.as_ref()?;
    Some(raster_reader.chart_name())
  }

  /// True if a heliport chart is currently open.
  pub fn heliport(&self) -> bool {
    self.heliport
  }

  pub fn transformation(&self) -> Option<&chart::Transformation> {
    Some(self.raster_reader.as_ref()?.transformation())
  }

  pub fn set_scale(&mut self, scale: f32) {
    self.display_info.ui_scale = scale;
  }

  pub fn set_night_mode(&mut self, dark: bool) {
    if self.display_info.night_mode != dark {
      self.display_info.night_mode = dark;
      self.request_image();
    }
  }

  pub fn set_show_bounds(&mut self, show_bounds: bool) {
    if self.display_info.show_bounds != show_bounds {
      self.display_info.show_bounds = show_bounds;
      self.base_mut().queue_redraw();
    }
  }

  pub fn goto_coord(&mut self, coord: geom::DD) {
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

    let pal_type = if self.display_info.night_mode {
      chart::PaletteType::Dark
    } else {
      chart::PaletteType::Light
    };

    let pos = self.display_info.origin;
    let size = self.base().get_size().into();
    let rect = geom::Rect { pos, size };
    let part = chart::ImagePart::new(rect, self.display_info.zoom, pal_type);
    raster_reader.read_image(part);
  }

  fn get_raster_reply(&self) -> Option<ChartImage> {
    let raster_reader = self.raster_reader.as_ref()?;
    let mut image_info = None;

    // Collect all chart replies to get to the most recent image.
    while let Some(reply) = raster_reader.get_reply() {
      match reply {
        chart::Reply::Image(part, texture) => {
          image_info = Some((part, texture.into()));
        }
        chart::Reply::Error(part, err) => {
          godot_error!("{err} @ {part:?}");
        }
      }
    }

    let (part, texture) = image_info?;
    Some(ChartImage { texture, part })
  }

  fn get_draw_info(&self) -> Option<(Gd<Texture2D>, Rect2)> {
    let chart_image = self.chart_image.as_ref()?;
    let zoom = self.display_info.zoom / chart_image.part.zoom;
    let pos = Vector2::from(chart_image.part.rect.pos) * zoom - self.display_info.origin.into();
    let size = chart_image.texture.get_size() * zoom;
    let rect = Rect2::new(pos, size);
    let texture = chart_image.texture.clone();
    Some((texture, rect))
  }

  fn get_raster_size(&self) -> Option<geom::Size> {
    let raster_reader = self.raster_reader.as_ref()?;
    Some(raster_reader.transformation().px_size())
  }

  fn correct_pos(&mut self, mut pos: geom::Pos) -> Option<geom::Pos> {
    let chart_size = self.get_raster_size()?;
    let max_size = chart_size * self.display_info.zoom as f64;
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
    let chart_size = self.get_raster_size()?;

    // Clamp the zoom value.
    let mut zoom = zoom.clamp(*util::ZOOM_RANGE.start(), *util::ZOOM_RANGE.end());

    let mut max_size = chart_size * zoom as f64;
    let widget_size: geom::Size = self.base().get_size().into();

    // Make sure the maximum chart size is not smaller than the widget.
    if max_size.w < widget_size.w {
      zoom = widget_size.w as f32 / chart_size.w as f32;
      max_size = chart_size * zoom as f64;
    }

    if max_size.h < widget_size.h {
      zoom = widget_size.h as f32 / chart_size.h as f32;
      max_size = chart_size * zoom as f64;
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
    if !self.display_info.show_bounds {
      return;
    }

    let Some(raster_reader) = &self.raster_reader else {
      return;
    };

    // Get the chart bounds polygon in pixels.
    let source = raster_reader.transformation().pixel_bounds();
    if source.is_empty() {
      return;
    }

    // Scale and translate the coordinates to the current view.
    let zoom = self.display_info.zoom as f64;
    let origin = self.display_info.origin.into();
    let mut dest = Vec::with_capacity(source.len() + 1);
    for point in source.iter() {
      let point = **point * zoom - origin;
      dest.push(point.into());
    }

    if dest.first() != dest.last() {
      // Close off the polygon.
      dest.push(*dest.first().unwrap());
    }

    // Draw it as a polyline.
    let mut this = self.base_mut();
    let packed = dest.into();
    let poly_draw = this.draw_polyline_ex(&packed, Color::MAGENTA);
    poly_draw.width(1.0).antialiased(true).done();
  }
}

#[godot_api]
impl IControl for ChartWidget {
  fn init(base: Base<Control>) -> Self {
    Self {
      base,
      raster_reader: None,
      chart_image: None,
      display_info: DisplayInfo::new(),
      heliport: false,
    }
  }

  fn on_notification(&mut self, what: ControlNotification) {
    if what != ControlNotification::RESIZED {
      return;
    }

    let rect: geom::Rect = self.base().get_rect().into();
    if self.chart_image.is_some() {
      // Correct the current zoom (may change based on widget size).
      if let Some((zoom, _)) = self.correct_zoom(self.display_info.zoom, Vector2::default()) {
        self.display_info.zoom = zoom;

        let pos = if rect.pos.x == self.display_info.ctl_rect.pos.x {
          // Recenter the chart.
          self.display_info.origin + self.display_info.ctl_rect.center() - rect.center()
        } else {
          // Side panel was toggled, just compensate for that.
          self.display_info.origin + (rect.pos.x - self.display_info.ctl_rect.pos.x, 0).into()
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
    self.display_info.ctl_rect = rect;
  }

  fn draw(&mut self) {
    if let Some((texture, rect)) = self.get_draw_info() {
      self.base_mut().draw_texture_rect(&texture, rect, false);
    };

    self.draw_bounds();
  }

  fn process(&mut self, _delta: f64) {
    let chart_image = self.get_raster_reply();
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
      if event.get_button_mask() == MouseButtonMask::LEFT && !self.display_info.touch.multi {
        let delta = event.get_screen_relative() / self.display_info.ui_scale;
        let pos = self.display_info.origin - delta.into();
        self.set_pos(pos);
      }
      return;
    }

    if let Ok(event) = event.clone().try_cast::<InputEventMouseButton>() {
      if event.is_pressed() {
        return;
      }

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
      }
      return;
    }

    if let Ok(event) = event.clone().try_cast::<InputEventScreenTouch>() {
      self.display_info.touch.update(event);
      return;
    }

    if let Ok(event) = event.try_cast::<InputEventMagnifyGesture>() {
      let Some(pos) = self.display_info.touch.pos else {
        return;
      };

      let factor = 1.0 - 2.0 * (1.0 - event.get_factor()) / self.display_info.ui_scale;
      let zoom = self.display_info.zoom * factor;
      self.set_zoom(zoom, pos);
    }
  }
}

struct ChartImage {
  texture: Gd<Texture2D>,
  part: chart::ImagePart,
}

struct DisplayInfo {
  touch: Touch,
  ui_scale: f32,
  ctl_rect: geom::Rect,
  origin: geom::Pos,
  zoom: f32,
  night_mode: bool,
  show_bounds: bool,
}

impl DisplayInfo {
  fn new() -> Self {
    Self {
      touch: Touch::default(),
      ui_scale: 1.0,
      ctl_rect: geom::Rect::default(),
      origin: geom::Pos::default(),
      zoom: 1.0,
      night_mode: false,
      show_bounds: false,
    }
  }
}

#[derive(Default)]
struct Touch {
  touch: [Option<Vector2>; 2],
  pos: Option<Vector2>,
  multi: bool,
}

impl Touch {
  fn update(&mut self, event: Gd<InputEventScreenTouch>) {
    let index = event.get_index();
    if !(0..=1).contains(&index) {
      self.pos = None;
      return;
    }

    let index = index as usize;
    if event.is_pressed() {
      self.touch[index] = Some(event.get_position());
    } else {
      self.touch[index] = None;
    }

    if let [Some(pt1), Some(pt2)] = &self.touch {
      self.pos = Some((*pt1 + *pt2) * 0.5);
    }

    // Toggle multi flag.
    if self.touch[0].is_some() == self.touch[1].is_some() {
      self.multi = self.touch[0].is_some();
    }
  }
}
