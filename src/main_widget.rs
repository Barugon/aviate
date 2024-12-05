use crate::{chart_widget, config, find_dialog, nasr, select_dialog, util};
use godot::{
  classes::{
    display_server::WindowMode, notify::ControlNotification, AcceptDialog, Button, CheckButton,
    Control, DisplayServer, FileDialog, HBoxContainer, IControl, InputEvent, InputEventKey, Label,
    OptionButton, PanelContainer, Window,
  },
  global::{HorizontalAlignment, Key, KeyModifierMask},
  prelude::*,
};
use std::path;

#[derive(GodotClass)]
#[class(base=Control)]
struct MainWidget {
  base: Base<Control>,
  config: config::Storage,
  chart_widget: OnReady<Gd<chart_widget::ChartWidget>>,
  chart_info: Option<(String, Vec<path::PathBuf>)>,
  find_button: OnReady<Gd<Button>>,
  airport_label: OnReady<Gd<Label>>,
  airport_reader: Option<nasr::AirportReader>,
  airport_infos: Option<Vec<nasr::AirportInfo>>,
  airport_status: AirportStatus,
}

#[godot_api]
impl MainWidget {
  #[func]
  fn toggle_sidebar(&self, toggle: bool) {
    let mut panel = self.get_child::<PanelContainer>("SidebarPanel");
    panel.set_visible(toggle);

    let mut button = self.get_child::<CheckButton>("SidebarButton");
    button.set_tooltip_text(if toggle {
      "Hide side panel"
    } else {
      "Show side panel"
    });
  }

  #[func]
  fn toggle_night_mode(&mut self, night_mode: bool) {
    self.chart_widget.bind_mut().set_night_mode(night_mode);
    self.config.set_night_mode(night_mode);
  }

  #[func]
  fn toggle_show_bounds(&mut self, night_mode: bool) {
    self.chart_widget.bind_mut().set_show_bounds(night_mode);
    self.config.set_show_bounds(night_mode);
  }

  #[func]
  fn find(&self) {
    if !self.find_button.is_visible() || self.dialog_is_visible() {
      return;
    }

    let mut dialog = self.get_child::<find_dialog::FindDialog>("FindDialog");
    if dialog.is_visible() {
      return;
    }

    dialog.call_deferred("show", &[]);
  }

  #[func]
  fn find_confirmed(&self, text: GString) {
    if let Some(airport_reader) = &self.airport_reader {
      let helicopter = self.chart_widget.bind().helicopter();
      airport_reader.search(text.to_string(), helicopter);
    }
  }

  #[func]
  fn select_zip_file(&self) {
    if self.dialog_is_visible() {
      return;
    }

    let mut dialog = self.get_child::<FileDialog>("FileDialog");
    let mut filters = PackedStringArray::new();
    filters.push("*.zip;Zip Files");
    dialog.set_filters(&filters);
    dialog.set_title("Open FAA Zip File");

    if let Some(folder) = self.get_asset_folder() {
      dialog.set_current_dir(&folder);
    }

    dialog.call_deferred("show", &[]);
  }

  #[func]
  fn zip_file_selected(&mut self, path: String) {
    match util::get_zip_info(&path) {
      Ok(info) => match info {
        util::ZipInfo::Chart(files) => {
          self.save_asset_folder(&path);

          if files.len() > 1 {
            self.select_chart(path, files);
          } else {
            self.open_chart(&path, files.first().and_then(|f| f.to_str()).unwrap());
          }
        }
        util::ZipInfo::Aero { csv, shp } => {
          self.save_asset_folder(&path);

          let csv = csv.to_str().unwrap();
          let shp = shp.to_str().unwrap();
          self.open_nasr(&path, csv, shp);
        }
      },
      Err(err) => {
        self.show_alert(err.as_ref());
      }
    }
  }

  #[func]
  fn item_selected(&mut self, index: u32) {
    if let Some((path, files)) = self.chart_info.take() {
      self.open_chart(&path, files[index as usize].to_str().unwrap());
    } else if let Some(infos) = self.airport_infos.take() {
      let coord = infos[index as usize].coord;
      self.chart_widget.bind_mut().goto_coord(coord);
    }
  }

  fn select_chart(&mut self, path: String, files: Vec<path::PathBuf>) {
    let mut dialog = self.get_child::<select_dialog::SelectDialog>("SelectDialog");
    dialog.set_title("Select Chart");

    let choices = files.iter().map(|f| util::stem_str(f).unwrap());
    dialog.bind_mut().show_choices(choices);

    self.chart_info = Some((path, files));
    self.airport_infos = None;
  }

  fn select_airport(&mut self, airports: Vec<nasr::AirportInfo>) {
    // It's possible to open a another dialog before the airport query is complete.
    if self.dialog_is_visible() {
      return;
    }

    let mut dialog = self.get_child::<select_dialog::SelectDialog>("SelectDialog");
    dialog.set_title("Select Airport");

    let choices = airports.iter().map(|a| a.desc.as_str());
    dialog.bind_mut().show_choices(choices);

    self.airport_infos = Some(airports);
    self.chart_info = None;
  }

  fn open_chart(&mut self, path: &str, file: &str) {
    let result = self.chart_widget.bind_mut().open_raster_reader(path, file);
    match result {
      Ok(()) => {
        if let Some(airport_reader) = &self.airport_reader {
          if let Some(transformation) = self.chart_widget.bind().transformation() {
            // Send the chart spatial reference to the airport reader.
            let proj4 = transformation.get_proj4();
            let bounds = transformation.chart_bounds();
            airport_reader.set_chart_spatial_ref(proj4, bounds);
          }
        }
      }
      Err(err) => {
        self.show_alert(err.as_ref());
      }
    }
  }

  fn open_nasr(&mut self, path: &str, csv: &str, _shp: &str) {
    // Concatenate the VSI prefix and the airport csv path.
    let path = ["/vsizip//vsizip/", path].concat();
    let path = path::Path::new(path.as_str());
    let path = path.join(csv).join("APT_BASE.csv");

    match nasr::AirportReader::new(path) {
      Ok(airport_reader) => {
        if let Some(transformation) = self.chart_widget.bind().transformation() {
          // Send the chart spatial reference to the airport reader.
          let proj4 = transformation.get_proj4();
          let bounds = transformation.chart_bounds();
          airport_reader.set_chart_spatial_ref(proj4, bounds);
        }
        self.airport_reader = Some(airport_reader);
      }
      Err(err) => {
        self.show_alert(err.as_ref());
      }
    }
  }

  fn show_alert(&self, text: &str) {
    let mut dialog = self.get_child::<AcceptDialog>("AlertDialog");
    dialog.set_text(text);
    dialog.reset_size();
    dialog.call_deferred("show", &[]);
  }

  fn get_asset_folder(&self) -> Option<GString> {
    let folder = self.config.get_asset_folder();
    if folder.is_some() {
      return folder;
    }

    Some(util::get_downloads_folder())
  }

  fn save_asset_folder(&mut self, path: &str) {
    if let Some(folder) = util::folder_gstring(path) {
      self.config.set_asset_folder(folder);
    }
  }

  /// Returns true if a dialog window is visible.
  fn dialog_is_visible(&self) -> bool {
    for child in self.base().get_children().iter_shared() {
      if let Ok(window) = child.try_cast::<Window>() {
        if window.is_exclusive() && window.is_visible() {
          return true;
        }
      }
    }
    false
  }

  /// Set the main window's size and position.
  fn setup_window(&mut self) -> f32 {
    let win_info = self.config.get_win_info();
    let mut display_server = DisplayServer::singleton();
    display_server.window_set_min_size(Vector2i { x: 800, y: 600 });

    // Godot doesn't handle hi-dpi.
    let dpi = display_server.screen_get_dpi();
    let scale = get_scale(dpi);
    if let Some(tree) = self.base().get_tree() {
      if let Some(mut root) = tree.get_root() {
        root.call_deferred("set_content_scale_factor", &[Variant::from(scale)]);
      }
    }

    if win_info.maxed {
      display_server.window_set_mode(WindowMode::MAXIMIZED);
      return scale;
    }

    if let Some(pos) = win_info.pos {
      display_server.window_set_position(pos.into());
    }

    if let Some(size) = win_info.size {
      display_server.window_set_size(size.into());
    }

    scale
  }

  fn get_child<T: Inherits<Node>>(&self, name: &str) -> Gd<T> {
    self.base().find_child(name).unwrap().cast()
  }
}

#[godot_api]
impl IControl for MainWidget {
  fn init(base: Base<Control>) -> Self {
    util::request_permissions();

    let airport_status = AirportStatus {
      index: nasr::AirportIndex::None,
      pending: false,
    };

    Self {
      base,
      config: config::Storage::new(),
      chart_widget: OnReady::manual(),
      chart_info: None,
      find_button: OnReady::manual(),
      airport_label: OnReady::manual(),
      airport_reader: None,
      airport_infos: None,
      airport_status,
    }
  }

  fn on_notification(&mut self, what: ControlNotification) {
    if what == ControlNotification::WM_CLOSE_REQUEST {
      let win_info = util::WinInfo::from_display(&DisplayServer::singleton());
      self.config.set_win_info(&win_info);
    }
  }

  fn ready(&mut self) {
    let scale = self.setup_window();

    // Get the chart widget.
    self.chart_widget.init(self.get_child("ChartWidget"));

    // Get the airport label.
    self.airport_label.init(self.get_child("AirportLabel"));

    // Get the find button.
    self.find_button.init(self.get_child("FindButton"));

    // Connect the find button.
    let callable = self.base().callable("find");
    self.find_button.connect("pressed", &callable);

    // Connect the sidebar button.
    let mut button = self.get_child::<CheckButton>("SidebarButton");
    button.connect("toggled", &self.base().callable("toggle_sidebar"));

    // Connect the open button.
    let mut button = self.get_child::<Button>("OpenButton");
    button.connect("pressed", &self.base().callable("select_zip_file"));

    // Read nite mode from the config.
    let night_mode = self.config.get_night_mode().unwrap_or(false);
    self.chart_widget.bind_mut().set_night_mode(night_mode);

    // Connect the night mode button
    let mut button = self.get_child::<CheckButton>("NightModeButton");
    button.set_pressed(night_mode);
    button.connect("toggled", &self.base().callable("toggle_night_mode"));

    // Read show bounds from the config.
    let show_bounds = self.config.get_show_bounds().unwrap_or(false);
    self.chart_widget.bind_mut().set_show_bounds(show_bounds);

    // Connect the show bounds button
    let mut button = self.get_child::<CheckButton>("BoundsButton");
    button.set_pressed(show_bounds);
    button.connect("toggled", &self.base().callable("toggle_show_bounds"));

    let title_property = StringName::from("theme_override_font_sizes/title_font_size");
    let title_size = Variant::from(18.0);

    // Setup the file dialog.
    let mut dialog = self.get_child::<FileDialog>("FileDialog");
    dialog.connect("file_selected", &self.base().callable("zip_file_selected"));
    dialog.set(&title_property, &title_size);

    #[cfg(target_os = "android")]
    dialog.set_root_subfolder("/storage/emulated/0");

    // The content scale hasn't been applied yet, so we need to account for it here.
    fixup_file_dialog(&mut dialog, (self.base().get_size().x / scale) as i32);

    // Setup the alert dialog.
    let mut dialog = self.get_child::<AcceptDialog>("AlertDialog");
    dialog.set(&title_property, &title_size);

    if let Some(label) = dialog.get_label() {
      let mut label = label;
      let property = "theme_override_colors/font_color";
      label.set(property, &Variant::from(Color::from_rgb(1.0, 0.4, 0.4)));
      label.set_horizontal_alignment(HorizontalAlignment::CENTER);
    }

    // Setup and connect the select dialog.
    let mut dialog = self.get_child::<select_dialog::SelectDialog>("SelectDialog");
    dialog.connect("selected", &self.base().callable("item_selected"));
    dialog.set(&title_property, &title_size);

    // Setup and connect the find dialog.
    let mut dialog = self.get_child::<find_dialog::FindDialog>("FindDialog");
    dialog.connect("confirmed", &self.base().callable("find_confirmed"));
    dialog.set(&title_property, &title_size);
  }

  fn process(&mut self, _delta: f64) {
    let Some(airport_reader) = &self.airport_reader else {
      return;
    };

    // Check if the index level has changed.
    let index = airport_reader.get_index_level();
    match self.airport_status.index {
      nasr::AirportIndex::None => {
        if index > nasr::AirportIndex::None {
          // Show the APT label.
          self.airport_label.set_visible(true);
          self.airport_status.index = nasr::AirportIndex::Basic;
        }
      }
      nasr::AirportIndex::Basic => {
        match index.cmp(&nasr::AirportIndex::Basic) {
          std::cmp::Ordering::Less => {
            // Hide the APT label.
            self.airport_label.set_visible(false);
            self.airport_status.index = nasr::AirportIndex::None;
          }
          std::cmp::Ordering::Greater => {
            // Show the find button.
            self.find_button.set_visible(true);
            self.airport_status.index = nasr::AirportIndex::Advanced;
          }
          std::cmp::Ordering::Equal => (),
        }
      }
      nasr::AirportIndex::Advanced => {
        if index < nasr::AirportIndex::Advanced {
          // Hide the find button.
          self.find_button.set_visible(false);
          self.airport_status.index = nasr::AirportIndex::Basic;
        }
      }
    }

    // Check if there are pending requests.
    let pending = airport_reader.request_count() > 0;
    if pending != self.airport_status.pending {
      // Set the airport label's color to indicate if its busy.
      let property = "theme_override_colors/font_color";
      let color = if pending {
        Color::from_rgb(1.0, 1.0, 0.0)
      } else {
        Color::from_rgb(0.5, 0.5, 0.5)
      };
      self.airport_label.set(property, &Variant::from(color));
      self.airport_status.pending = pending;
    }

    // Collect airport replies.
    let mut airport_infos = None;
    while let Some(reply) = airport_reader.get_reply() {
      match reply {
        nasr::AirportReply::Airport(info) => {
          self.chart_widget.bind_mut().goto_coord(info.coord);
        }
        nasr::AirportReply::Nearby(_infos) => (),
        nasr::AirportReply::Search(infos) => {
          if infos.len() > 1 {
            airport_infos = Some(infos);
          } else {
            let coord = infos.first().unwrap().coord;
            self.chart_widget.bind_mut().goto_coord(coord);
          }
        }
        nasr::AirportReply::Error(err) => {
          self.show_alert(err.as_ref());
        }
      }
    }

    if let Some(airport_infos) = airport_infos {
      self.select_airport(airport_infos);
    }
  }

  fn shortcut_input(&mut self, event: Gd<InputEvent>) {
    let event_key = event.cast::<InputEventKey>();
    match event_key.get_keycode() {
      Key::F => {
        if cmd_or_ctrl(&event_key) {
          self.find();
        }
      }
      Key::O => {
        if cmd_or_ctrl(&event_key) {
          self.select_zip_file();
        }
      }
      _ => (),
    }
  }
}

struct AirportStatus {
  index: nasr::AirportIndex,
  pending: bool,
}

fn fixup_file_dialog(file_dialog: &mut Gd<FileDialog>, max_width: i32) {
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

  // Locations.
  let mut hbox = children.at(8).try_cast::<HBoxContainer>().unwrap();
  hbox.set_visible(false);

  let hbox = vbox_children.at(3).try_cast::<HBoxContainer>().unwrap();
  let children = hbox.get_children();

  // Filters.
  let mut button = children.at(2).try_cast::<OptionButton>().unwrap();
  button.set_visible(false);

  // Set the initial dialog size.
  file_dialog.set_size(Vector2i::new(500.min(max_width), 400));
}

/// Test if a key event has CMD or CTRL modifiers.
fn cmd_or_ctrl(event: &Gd<InputEventKey>) -> bool {
  event.get_modifiers_mask() == KeyModifierMask::CTRL
    || event.get_modifiers_mask() == KeyModifierMask::CMD_OR_CTRL
}

/// Get an appropriate scale value.
fn get_scale(dpi: i32) -> f32 {
  let scale = if cfg!(target_os = "android") {
    // Use 160 dpi as the base for Android.
    dpi as f32 / 160.0
  } else {
    // Use 120 dpi as the base for desktop.
    dpi as f32 / 120.0
  };

  // Quantize to 0.5.
  let scale = (scale * 2.0).trunc() * 0.5;

  // Make sure the final value doesn't fall below 1.0.
  scale.max(1.0)
}
