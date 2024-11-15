use crate::{chart_widget::ChartWidget, select_dialog::SelectDialog, util};
use std::path;

use godot::{
  classes::{AcceptDialog, Button, Control, FileDialog, HBoxContainer, IControl, PanelContainer},
  global::HorizontalAlignment,
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Control)]
struct MainWidget {
  base: Base<Control>,
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
  fn open_zip_file(&self) {
    if let Some(node) = self.base().find_child("FileDialog") {
      let mut file_dialog = node.cast::<FileDialog>();
      let property = "theme_override_font_sizes/title_font_size";
      file_dialog.set(property, &Variant::from(16.0));

      if let Some(folder) = dirs::download_dir() {
        if let Some(folder) = folder.to_str() {
          file_dialog.set_current_dir(folder);
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
          if files.len() > 1 {
            self.select_chart(&files);
            self.chart_info = Some((path, files));
          } else {
            self.open_chart(&path, files.first().and_then(|f| f.to_str()).unwrap());
          }
        }
        util::ZipInfo::Aero { csv, shp } => {
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
    let mut err = None;
    if let Some(node) = self.base().find_child("ChartWidget") {
      let mut chart_widget = node.cast::<ChartWidget>();
      let mut chart_widget = chart_widget.bind_mut();
      err = chart_widget.open_chart(path, file).err();
    }

    if let Some(err) = err.take() {
      self.show_alert(err.as_ref());
    }
  }

  fn open_nasr(&mut self, path: &str, csv: &str, _shp: &str) {
    let mut err = None;
    if let Some(node) = self.base().find_child("ChartWidget") {
      let mut chart_widget = node.cast::<ChartWidget>();
      let mut chart_widget = chart_widget.bind_mut();
      err = chart_widget.open_airport_csv(path, csv).err();
    };

    if let Some(err) = err.take() {
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
}

#[godot_api]
impl IControl for MainWidget {
  fn init(base: Base<Control>) -> Self {
    Self {
      base,
      chart_info: None,
    }
  }

  fn ready(&mut self) {
    let this = self.base();

    // Connect the sidebar button.
    if let Some(mut node) = this.find_child("SidebarButton") {
      node.connect("toggled", &this.callable("toggle_sidebar"));
    }

    // Connect the open button.
    if let Some(mut node) = this.find_child("OpenButton") {
      node.connect("pressed", &this.callable("open_zip_file"));
    }

    // Setup the file dialog.
    if let Some(node) = this.find_child("FileDialog") {
      let mut file_dialog = node.cast::<FileDialog>();
      file_dialog.connect("file_selected", &this.callable("zip_file_selected"));

      let vbox = file_dialog.get_vbox().unwrap();
      hide_buttons(vbox.upcast());
    }

    // Connect the select dialog.
    if let Some(mut node) = this.find_child("SelectDialog") {
      node.connect("selected", &this.callable("chart_selected"));
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
