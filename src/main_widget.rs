use crate::{chart_widget::ChartWidget, config, nasr, select_dialog::SelectDialog, util};
use godot::{
  classes::{
    AcceptDialog, Button, CheckButton, Control, DisplayServer, FileDialog, HBoxContainer, IControl,
    PanelContainer,
  },
  global::HorizontalAlignment,
  prelude::*,
};
use std::path;

#[derive(GodotClass)]
#[class(base=Control)]
struct MainWidget {
  base: Base<Control>,
  config: Option<config::Storage>,
  chart_widget: OnReady<Gd<ChartWidget>>,
  chart_info: Option<(String, Vec<path::PathBuf>)>,
}

#[godot_api]
impl MainWidget {
  #[func]
  fn toggle_sidebar(&self, toggle: bool) {
    if let Some(node) = self.base().find_child("SidebarPanel") {
      let mut sidebar = node.cast::<PanelContainer>();
      sidebar.set_visible(toggle);
    }
  }

  #[func]
  fn toggle_night_mode(&mut self, night_mode: bool) {
    self.chart_widget.bind_mut().set_night_mode(night_mode);
    if let Some(config) = &mut self.config {
      config.set_night_mode(night_mode);
    }
  }

  #[func]
  fn open_zip_file(&self) {
    if let Some(node) = self.base().find_child("FileDialog") {
      let mut file_dialog = node.cast::<FileDialog>();
      let property = "theme_override_font_sizes/title_font_size";
      file_dialog.set(property, &Variant::from(16.0));

      if let Some(config) = &self.config {
        if let Some(folder) = config.get_asset_folder() {
          file_dialog.set_current_dir(&folder);
        }
      }

      file_dialog.show();
    }
  }

  #[func]
  fn zip_file_selected(&mut self, path: String) {
    // The file dialog needs to be hidden first or it will generate an error if the alert dialog is shown.
    if let Some(node) = self.base().find_child("FileDialog") {
      let mut file_dialog = node.cast::<FileDialog>();
      file_dialog.hide();
    }

    match util::get_zip_info(&path) {
      Ok(info) => match info {
        util::ZipInfo::Chart(files) => {
          self.save_asset_folder(&path);

          if files.len() > 1 {
            self.select_chart(&files);
            self.chart_info = Some((path, files));
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
  fn chart_selected(&mut self, index: u32) {
    if let Some((path, files)) = self.chart_info.take() {
      self.open_chart(&path, files[index as usize].to_str().unwrap());
    }
  }

  fn select_chart(&self, files: &[path::PathBuf]) {
    if let Some(node) = self.base().find_child("SelectDialog") {
      let mut select_dialog = node.cast::<SelectDialog>();
      select_dialog.set_title("Select Chart");

      let choices = files.iter().map(|f| util::stem_str(f).unwrap());
      select_dialog.bind_mut().show_choices(choices);
    }
  }

  fn open_chart(&mut self, path: &str, file: &str) {
    let result = self.chart_widget.bind_mut().open_raster_reader(path, file);
    if let Err(err) = result {
      self.show_alert(err.as_ref());
    }
  }

  fn open_nasr(&mut self, path: &str, csv: &str, _shp: &str) {
    let result = self.chart_widget.bind_mut().open_airport_reader(path, csv);
    if let Err(err) = result {
      self.show_alert(err.as_ref());
    }
  }

  fn show_alert(&self, text: &str) {
    if let Some(child) = self.base().find_child("AlertDialog") {
      let mut alert_dialog = child.cast::<AcceptDialog>();
      let property = "theme_override_font_sizes/title_font_size";
      alert_dialog.set(property, &Variant::from(16.0));

      if let Some(label) = alert_dialog.get_label() {
        let mut label = label;
        let property = "theme_override_colors/font_color";
        label.set(property, &Variant::from(Color::from_rgb(1.0, 0.4, 0.4)));
        label.set_horizontal_alignment(HorizontalAlignment::CENTER);
      }

      alert_dialog.set_text(text);
      alert_dialog.reset_size();
      alert_dialog.show();
      return;
    }
    godot_error!("{text}");
  }

  fn save_asset_folder(&mut self, path: &str) {
    if let Some(config) = &mut self.config {
      if let Some(folder) = util::folder_string(path) {
        config.set_asset_folder(folder);
      }
    }
  }
}

#[godot_api]
impl IControl for MainWidget {
  fn init(base: Base<Control>) -> Self {
    Self {
      base,
      config: config::Storage::new(false),
      chart_widget: OnReady::manual(),
      chart_info: None,
    }
  }

  fn ready(&mut self) {
    DisplayServer::singleton().window_set_min_size(Vector2i { x: 600, y: 400 });

    // Get the chart widget.
    let node = self.base().find_child("ChartWidget").unwrap();
    self.chart_widget.init(node.cast());

    // Read nite mode from the config.
    let night_mode = self.config.as_ref().and_then(|c| c.get_night_mode());
    let night_mode = night_mode.unwrap_or(false);
    self.chart_widget.bind_mut().set_night_mode(night_mode);

    let this = self.base();

    // Connect the sidebar button.
    let mut node = self.base().find_child("SidebarButton").unwrap();
    node.connect("toggled", &this.callable("toggle_sidebar"));

    // Connect the open button.
    let mut node = self.base().find_child("OpenButton").unwrap();
    node.connect("pressed", &this.callable("open_zip_file"));

    // Setup the file dialog.
    let node = self.base().find_child("FileDialog").unwrap();
    let mut node = node.cast::<FileDialog>();
    node.connect("file_selected", &this.callable("zip_file_selected"));
    hide_buttons(node.get_vbox().unwrap().upcast());

    // Connect the night mode button
    let node = this.find_child("NightModeButton").unwrap();
    let mut node = node.cast::<CheckButton>();
    node.set_pressed(night_mode);
    node.connect("toggled", &this.callable("toggle_night_mode"));

    // Connect the select dialog.
    let mut node = this.find_child("SelectDialog").unwrap();
    node.connect("selected", &this.callable("chart_selected"));
  }

  fn process(&mut self, _delta: f64) {
    let chart_widget = self.chart_widget.bind();
    let Some(airport_reader) = chart_widget.airport_reader() else {
      return;
    };

    while let Some(reply) = airport_reader.get_reply() {
      match reply {
        nasr::AirportReply::Airport(_info) => (),
        nasr::AirportReply::Nearby(_infos) => (),
        nasr::AirportReply::Search(_infos) => (),
        nasr::AirportReply::Error(err) => godot_error!("{err}"),
      }
    }
  }
}

/// Hide the forward and back buttons.
fn hide_buttons(node: Gd<Node>) {
  if let Ok(hbox) = node.get_children().at(0).try_cast::<HBoxContainer>() {
    let children = hbox.get_children();

    // Back button.
    if let Ok(button) = children.at(0).try_cast::<Button>() {
      let mut button = button;
      button.set_visible(false);
    }

    // Forward button.
    if let Ok(button) = children.at(1).try_cast::<Button>() {
      let mut button = button;
      button.set_visible(false);
    }
  }
}
