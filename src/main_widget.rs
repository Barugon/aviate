use crate::{chart_widget, config, find_dialog, geom, info_dialog, nasr::airport, select_dialog, util};
use godot::{
  classes::{
    AcceptDialog, Button, CheckBox, CheckButton, Control, DisplayServer, FileDialog, HBoxContainer, IControl, Input,
    InputEvent, InputEventKey, Label, MarginContainer, OptionButton, PanelContainer, Tree, Window,
    display_server::WindowMode, input::CursorShape, notify::ControlNotification,
  },
  global::{HorizontalAlignment, Key, KeyModifierMask},
  prelude::*,
};
use std::{cmp, path};

#[derive(GodotClass)]
#[class(base=Control)]
struct MainWidget {
  base: Base<Control>,
  config: config::Storage,
  chart_widget: OnReady<Gd<chart_widget::ChartWidget>>,
  chart_info: Option<(String, Vec<path::PathBuf>)>,
  find_button: OnReady<Gd<Button>>,
  airport_label: OnReady<Gd<Label>>,
  airport_reader: Option<airport::Reader>,
  airport_summaries: Option<Vec<airport::Summary>>,
  airport_status: AirportStatus,
}

#[godot_api]
impl MainWidget {
  #[func]
  fn toggle_sidebar(&self, visible: bool) {
    let mut panel = self.get_child::<PanelContainer>("SidebarPanel");
    if panel.is_visible() != visible {
      panel.set_visible(visible);

      let mut button = self.get_child::<CheckButton>("SidebarButton");
      button.set_tooltip_text(if visible { "Hide side panel" } else { "Show side panel" });
    }
  }

  #[func]
  fn toggle_night_mode(&mut self, night_mode: bool) {
    self.chart_widget.bind_mut().set_night_mode(night_mode);
    self.config.set_night_mode(night_mode);
  }

  #[func]
  fn toggle_show_bounds(&mut self, show_bounds: bool) {
    self.chart_widget.bind_mut().set_show_bounds(show_bounds);
    self.config.set_show_bounds(show_bounds);
  }

  #[func]
  fn find_clicked(&self) {
    if !self.find_button.is_visible() || self.get_visible_dialog().is_some() {
      return;
    }

    let mut dialog = self.get_child::<find_dialog::FindDialog>("FindDialog");
    if dialog.is_visible() {
      return;
    }

    dialog.reset_size();
    dialog.call_deferred("show", &[]);
  }

  #[func]
  fn find_confirmed(&self, text: GString) {
    if let Some(airport_reader) = &self.airport_reader {
      let heliport = self.chart_widget.bind().heliport();
      airport_reader.search(text.to_string(), heliport);
    }
  }

  #[func]
  fn open_zip_file_clicked(&self) {
    if self.get_visible_dialog().is_some() {
      return;
    }

    let mut dialog = self.get_child::<FileDialog>("FileDialog");
    let mut filters = PackedStringArray::new();
    filters.push("*.zip;Zip Files");
    dialog.set_filters(&filters);
    dialog.set_title("Open FAA Zip File");
    dialog.set_current_dir(&self.get_asset_folder());

    // Set the dialog size.
    let width = cmp::min(500, self.base().get_size().x as i32);
    let height = dialog.get_size().y;
    dialog.set_size(Vector2i::new(width, height));

    dialog.call_deferred("show", &[]);
  }

  #[func]
  fn open_zip_file_confirmed(&mut self, path: String) {
    let info = match util::get_zip_info(path::Path::new(&path)) {
      Ok(info) => info,
      Err(err) => {
        self.show_alert(err.as_ref());
        return;
      }
    };

    match info {
      util::ZipInfo::Chart(files) => {
        self.save_asset_folder(&path);

        if files.len() > 1 {
          self.select_chart(path, files);
        } else if let Some(file) = files.first()
          && let Some(file) = file.to_str()
        {
          self.open_chart(&path, file);
        }
      }
      util::ZipInfo::Aero { csv, shp } => {
        self.save_asset_folder(&path);
        self.open_nasr(&path, csv, shp);
      }
    }
  }

  #[func]
  fn select_item_confirmed(&mut self, index: u32) {
    let index = index as usize;
    if let Some((path, files)) = self.chart_info.take()
      && let Some(file) = files.get(index)
      && let Some(file) = file.to_str()
    {
      self.open_chart(&path, file);
    }

    if let Some(summaries) = self.airport_summaries.take()
      && let Some(summary) = summaries.get(index)
    {
      let coord = summary.coord();
      self.chart_widget.bind_mut().goto_coord(coord);
    }
  }

  #[func]
  fn select_info_confirmed(&mut self, index: u32) {
    if let Some(summaries) = self.airport_summaries.take()
      && let Some(summary) = summaries.into_iter().nth(index as usize)
      && let Some(airport_reader) = &self.airport_reader
    {
      airport_reader.detail(summary);
    };
  }

  #[func]
  fn goto_coord(&mut self, var: Variant) {
    if let Some(coord) = geom::Coord::from_variant(var) {
      self.chart_widget.bind_mut().goto_coord(coord.into());
    };
  }

  fn select_chart(&mut self, path: String, files: Vec<path::PathBuf>) {
    let mut dialog = self.get_child::<select_dialog::SelectDialog>("SelectDialog");
    let choices = files.iter().map(|f| util::stem_str(f).map(|s| s.into()));
    dialog.bind_mut().show_choices(choices, "Select Chart", " OK ", false);

    self.chart_info = Some((path, files));
    self.airport_summaries = None;
  }

  fn select_airport(&mut self, airports: Vec<airport::Summary>) {
    // It's possible to open a another dialog before the airport query is complete.
    if self.get_visible_dialog().is_some() {
      return;
    }

    let mut dialog = self.get_child::<select_dialog::SelectDialog>("SelectDialog");
    let choices = airports.iter().map(|a| Some(a.get_text().into()));
    dialog.bind_mut().show_choices(choices, "Select Airport", "Go To", true);

    self.airport_summaries = Some(airports);
    self.chart_info = None;
  }

  fn open_chart(&mut self, path: &str, file: &str) {
    let result = self.chart_widget.bind_mut().open_chart_reader(path, file);
    match result {
      Ok(()) => (),
      Err(err) => {
        self.show_alert(err.as_ref());
        return;
      }
    }

    if let Some(airport_reader) = &self.airport_reader
      && let Some(transformation) = self.chart_widget.bind().transformation()
    {
      // Send the chart spatial reference to the airport reader.
      let proj4 = transformation.get_proj4();
      let bounds = transformation.get_chart_bounds();
      airport_reader.set_chart_spatial_ref(proj4, bounds);
    }

    if let Some(chart_name) = self.chart_widget.bind().chart_name() {
      let mut chart_label = self.get_child::<Label>("ChartLabel");
      chart_label.set_text(chart_name);

      let mut status = self.get_child::<HBoxContainer>("ChartStatus");
      status.set_visible(true);
    }
  }

  fn open_nasr(&mut self, path: &str, csv: path::PathBuf, _shp: path::PathBuf) {
    // Concatenate the VSI prefix and the airport zip path.
    let path = path::PathBuf::from(["/vsizip/", path].concat()).join(csv);

    let airport_reader = match airport::Reader::new(&path) {
      Ok(airport_reader) => airport_reader,
      Err(err) => {
        self.show_alert(err.as_ref());
        return;
      }
    };

    if let Some(trans) = self.chart_widget.bind().transformation() {
      // Send the chart spatial reference and bounds to the airport reader.
      airport_reader.set_chart_spatial_ref(trans.get_proj4(), trans.get_chart_bounds());
    }

    self.airport_reader = Some(airport_reader);
  }

  fn show_airport_detail(&self, detail: Box<airport::Detail>) {
    // It's possible to open a another dialog before the airport detail query is complete.
    if self.get_visible_dialog().is_some() {
      return;
    }

    let title = format!("{} ({})", detail.summary().name(), detail.summary().id());
    let text = detail.get_text();
    let coord = detail.summary().coord();
    let mut dialog = self.get_child::<info_dialog::InfoDialog>("InfoDialog");
    dialog.bind_mut().show_info(&title, &text, coord);
  }

  fn show_alert(&self, text: &str) {
    if let Some(mut dialog) = self.get_visible_dialog() {
      dialog.call_deferred("hide", &[]);
    }

    let mut dialog = self.get_child::<AcceptDialog>("AlertDialog");
    dialog.set_text(text);
    dialog.reset_size();
    dialog.call_deferred("show", &[]);
  }

  fn get_asset_folder(&self) -> GString {
    let folder = self.config.get_asset_folder();
    folder.unwrap_or(util::get_downloads_folder())
  }

  fn save_asset_folder(&mut self, path: &str) {
    if let Some(folder) = util::folder_gstring(path) {
      self.config.set_asset_folder(folder);
    }
  }

  /// Returns a dialog window if one is visible.
  fn get_visible_dialog(&self) -> Option<Gd<Window>> {
    for child in self.base().get_children().iter_shared() {
      if let Ok(window) = child.try_cast::<Window>()
        && window.is_exclusive()
        && window.is_visible()
      {
        return Some(window);
      }
    }
    None
  }

  /// Set the main window's size and position.
  fn setup_window(&mut self) {
    let win_info = self.config.get_win_info();
    let mut display_server = DisplayServer::singleton();

    #[cfg(not(target_os = "android"))]
    display_server.window_set_min_size(Vector2i { x: 800, y: 600 });

    if win_info.maxed {
      display_server.window_set_mode(WindowMode::MAXIMIZED);
      return;
    }

    if let Some(pos) = win_info.pos {
      display_server.window_set_position(pos.into());
    }

    if let Some(size) = win_info.size {
      display_server.window_set_size(size.into());
    }
  }

  fn get_child<T: Inherits<Node>>(&self, name: &str) -> Gd<T> {
    self.base().find_child(name).unwrap().cast()
  }
}

#[godot_api]
impl IControl for MainWidget {
  fn init(base: Base<Control>) -> Self {
    util::request_permissions();
    Self {
      base,
      config: config::Storage::new(),
      chart_widget: OnReady::manual(),
      chart_info: None,
      find_button: OnReady::manual(),
      airport_label: OnReady::manual(),
      airport_reader: None,
      airport_summaries: None,
      airport_status: AirportStatus::default(),
    }
  }

  fn on_notification(&mut self, what: ControlNotification) {
    if what == ControlNotification::WM_CLOSE_REQUEST && cfg!(not(target_os = "android")) {
      let win_info = util::WinInfo::from_display(&DisplayServer::singleton());
      self.config.set_win_info(&win_info);
    }
  }

  fn ready(&mut self) {
    // Godot doesn't handle hi-dpi automatically.
    let dpi = DisplayServer::singleton().screen_get_dpi();
    let scale = get_scale(dpi);
    if let Some(tree) = self.base().get_tree()
      && let Some(mut root) = tree.get_root()
    {
      root.call_deferred("set_content_scale_factor", vslice![scale]);
    }

    // Set the main window's size and position.
    self.setup_window();

    // Get the chart widget.
    self.chart_widget.init(self.get_child("ChartWidget"));
    self.chart_widget.bind_mut().set_scale(scale);

    // Get the airport label.
    self.airport_label.init(self.get_child("AirportLabel"));

    // Get the find button.
    self.find_button.init(self.get_child("FindButton"));

    // Connect the find button.
    let callable = self.base().callable("find_clicked");
    self.find_button.connect("pressed", &callable);

    // Connect the sidebar button.
    let mut button = self.get_child::<CheckButton>("SidebarButton");
    let callable = self.base().callable("toggle_sidebar");
    button.connect("toggled", &callable);

    // Connect the open button.
    let mut button = self.get_child::<Button>("OpenButton");
    let callable = self.base().callable("open_zip_file_clicked");
    button.connect("pressed", &callable);

    // Read nite mode from the config.
    let night_mode = self.config.get_night_mode().unwrap_or(false);
    self.chart_widget.bind_mut().set_night_mode(night_mode);

    // Connect the night mode button
    let mut button = self.get_child::<CheckBox>("NightModeButton");
    let callable = self.base().callable("toggle_night_mode");
    button.set_pressed(night_mode);
    button.connect("toggled", &callable);

    // Read show bounds from the config.
    let show_bounds = self.config.get_show_bounds().unwrap_or(false);
    self.chart_widget.bind_mut().set_show_bounds(show_bounds);

    // Connect the show bounds button
    let mut button = self.get_child::<CheckBox>("BoundsButton");
    let callable = self.base().callable("toggle_show_bounds");
    button.set_pressed(show_bounds);
    button.connect("toggled", &callable);

    let title_property = StringName::from("theme_override_font_sizes/title_font_size");
    let title_size = Variant::from(18.0);

    // Setup the file dialog.
    let mut dialog = self.get_child::<FileDialog>("FileDialog");
    let callable = &self.base().callable("open_zip_file_confirmed");
    dialog.connect("file_selected", callable);
    dialog.set(&title_property, &title_size);
    fixup_file_dialog(&mut dialog);

    // Setup the alert dialog.
    let mut dialog = self.get_child::<AcceptDialog>("AlertDialog");
    dialog.set(&title_property, &title_size);

    if let Some(label) = dialog.get_label() {
      let mut label = label;
      label.set_horizontal_alignment(HorizontalAlignment::CENTER);
    }

    // Setup and connect the select dialog.
    let mut dialog = self.get_child::<select_dialog::SelectDialog>("SelectDialog");
    dialog.set(&title_property, &title_size);

    let callable = self.base().callable("select_item_confirmed");
    dialog.connect("item_confirmed", &callable);

    let callable = self.base().callable("select_info_confirmed");
    dialog.connect("info_confirmed", &callable);

    // Setup and connect the find dialog.
    let mut dialog = self.get_child::<find_dialog::FindDialog>("FindDialog");
    let callable = self.base().callable("find_confirmed");
    dialog.connect("confirmed", &callable);
    dialog.set(&title_property, &title_size);

    // Setup and connect the airport info dialog.
    let mut dialog = self.get_child::<info_dialog::InfoDialog>("InfoDialog");
    let callable = self.base().callable("goto_coord");
    dialog.connect("confirmed", &callable);
  }

  fn process(&mut self, _delta: f64) {
    let Some(airport_reader) = &self.airport_reader else {
      return;
    };

    if !self.airport_status.reader {
      self.airport_label.set_visible(true);
      self.airport_status.reader = true;
    }

    // Check if the indexing has changed.
    let indexed = airport_reader.has_chart_transformation();
    if self.airport_status.indexed != indexed {
      self.find_button.set_visible(indexed);
      self.airport_status.indexed = indexed;
    }

    // Check if there are pending requests.
    let pending = airport_reader.request_count() > 0;
    if self.airport_status.pending != pending {
      // Set the airport label's color to indicate if its busy.
      let property = "theme_override_colors/font_color";
      let (color, cursor) = if pending {
        (Color::from_rgb(1.0, 1.0, 0.0), CursorShape::BUSY)
      } else {
        (Color::from_rgb(0.5, 0.5, 0.5), CursorShape::ARROW)
      };

      Input::singleton().set_default_cursor_shape_ex().shape(cursor).done();
      self.airport_label.set(property, &Variant::from(color));
      self.airport_status.pending = pending;
    }

    // Get the most recent airport reply.
    let mut last_reply = None;
    loop {
      let reply = airport_reader.get_reply();
      if reply.is_none() {
        break;
      }
      last_reply = reply;
    }

    if let Some(reply) = last_reply {
      match reply {
        airport::Reply::Airport(summary) => self.select_airport(vec![summary]),
        airport::Reply::Detail(detail) => self.show_airport_detail(detail),
        airport::Reply::Nearby(_summaries) => (),
        airport::Reply::Search(summaries) => self.select_airport(summaries),
        airport::Reply::Error(err) => self.show_alert(err.as_ref()),
      }
    }
  }

  fn shortcut_input(&mut self, event: Gd<InputEvent>) {
    let Ok(key_event) = event.try_cast::<InputEventKey>() else {
      return;
    };

    if !cmd_or_ctrl(&key_event) {
      return;
    }

    match key_event.get_keycode() {
      Key::F => {
        self.find_clicked();
      }
      Key::O => {
        self.open_zip_file_clicked();
      }
      Key::Q => {
        if let Some(mut tree) = self.base().get_tree() {
          tree.quit();
        }
      }
      _ => (),
    }
  }
}

#[derive(Default)]
struct AirportStatus {
  reader: bool,
  indexed: bool,
  pending: bool,
}

/// Remove unnecessary widgets from the file dialog.
fn fixup_file_dialog(file_dialog: &mut Gd<FileDialog>) {
  let vbox = file_dialog.get_vbox().unwrap();
  let vbox_children = vbox.get_children();
  let hbox = vbox_children.at(0).try_cast::<HBoxContainer>().unwrap();
  let children = hbox.get_children();

  // Back button.
  let mut button = children.at(0).try_cast::<Button>().unwrap();
  button.set_visible(false);

  // Forward button.
  let mut button = children.at(1).try_cast::<Button>().unwrap();
  button.set_visible(false);

  // Hidden files button.
  let mut button = children.at(7).try_cast::<Button>().unwrap();
  button.set_visible(false);

  // Filter button.
  let mut button = children.at(8).try_cast::<Button>().unwrap();
  button.set_visible(false);

  // Locations.
  let mut hbox = children.at(9).try_cast::<HBoxContainer>().unwrap();
  hbox.set_visible(false);

  // Tree theme overrides.
  let container = vbox_children.at(2).try_cast::<MarginContainer>().unwrap();
  let mut tree = container.get_children().at(0).try_cast::<Tree>().unwrap();
  tree.add_theme_constant_override("draw_guides", 0);
  tree.add_theme_constant_override("v_separation", 2);

  #[cfg(target_os = "android")]
  util::hide_hover(&mut tree);

  let hbox = vbox_children.at(4).try_cast::<HBoxContainer>().unwrap();
  let children = hbox.get_children();

  // Filters.
  let mut button = children.at(2).try_cast::<OptionButton>().unwrap();
  button.set_visible(false);

  // Set the root subfolder to shared storage on Android.
  #[cfg(target_os = "android")]
  file_dialog.set_root_subfolder("/storage/emulated/0");
}

/// Test if a key event has CMD or CTRL modifiers.
fn cmd_or_ctrl(event: &Gd<InputEventKey>) -> bool {
  event.get_modifiers_mask() == KeyModifierMask::CTRL || event.get_modifiers_mask() == KeyModifierMask::CMD_OR_CTRL
}

/// Get an appropriate scale value.
fn get_scale(dpi: i32) -> f32 {
  // Use 140 as the base DPI.
  let scale = dpi as f32 / 140.0;

  // Quantize to 0.5.
  let scale = (scale * 2.0).trunc() * 0.5;

  // Make sure the scale doesn't go below 1.0.
  scale.max(1.0)
}
