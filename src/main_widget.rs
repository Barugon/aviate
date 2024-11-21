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
    let mut child = self.get_child::<PanelContainer>("SidebarPanel");
    child.set_visible(toggle);

    let mut child = self.get_child::<CheckButton>("SidebarButton");
    child.set_tooltip_text(if toggle {
      "Hide side panel"
    } else {
      "Show side panel"
    });
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
    let mut file_dialog = self.get_child::<FileDialog>("FileDialog");
    let property = "theme_override_font_sizes/title_font_size";
    file_dialog.set(property, &Variant::from(16.0));

    if let Some(folder) = self.get_asset_folder() {
      file_dialog.set_current_dir(&folder);
    }

    file_dialog.show();
  }

  #[func]
  fn zip_file_selected(&mut self, path: String) {
    // The file dialog needs to be hidden first or it will generate an error if the alert dialog is shown.
    self.get_child::<FileDialog>("FileDialog").hide();

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
    let mut select_dialog = self.get_child::<SelectDialog>("SelectDialog");
    select_dialog.set_title("Select Chart");

    let choices = files.iter().map(|f| util::stem_str(f).unwrap());
    select_dialog.bind_mut().show_choices(choices);
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
    let mut alert_dialog = self.get_child::<AcceptDialog>("AlertDialog");
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
  }

  fn get_asset_folder(&self) -> Option<String> {
    if let Some(config) = &self.config {
      let folder = config.get_asset_folder();
      if folder.is_some() {
        return folder;
      }
    }

    Some(dirs::download_dir()?.to_str()?.to_owned())
  }

  fn save_asset_folder(&mut self, path: &str) {
    if let Some(config) = &mut self.config {
      if let Some(folder) = util::folder_string(path) {
        config.set_asset_folder(folder);
      }
    }
  }

  fn get_child<T: Inherits<Node>>(&self, name: &str) -> Gd<T> {
    self.base().find_child(name).unwrap().cast()
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

  // fn on_notification(&mut self, _what: ControlNotification) {}

  fn ready(&mut self) {
    DisplayServer::singleton().window_set_min_size(Vector2i { x: 600, y: 400 });

    // Get the chart widget.
    self.chart_widget.init(self.get_child("ChartWidget"));

    // Read nite mode from the config.
    let night_mode = self.config.as_ref().and_then(|c| c.get_night_mode());
    let night_mode = night_mode.unwrap_or(false);
    self.chart_widget.bind_mut().set_night_mode(night_mode);

    // Connect the sidebar button.
    let mut child = self.get_child::<CheckButton>("SidebarButton");
    child.connect("toggled", &self.base().callable("toggle_sidebar"));

    // Connect the open button.
    let mut child = self.get_child::<Button>("OpenButton");
    child.connect("pressed", &self.base().callable("open_zip_file"));

    // Setup the file dialog.
    let mut child = self.get_child::<FileDialog>("FileDialog");
    child.connect("file_selected", &self.base().callable("zip_file_selected"));
    hide_buttons(child.get_vbox().unwrap().upcast());

    // Connect the night mode button
    let mut child = self.get_child::<CheckButton>("NightModeButton");
    child.set_pressed(night_mode);
    child.connect("toggled", &self.base().callable("toggle_night_mode"));

    // Connect the select dialog.
    let mut child = self.get_child::<SelectDialog>("SelectDialog");
    child.connect("selected", &self.base().callable("chart_selected"));
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
