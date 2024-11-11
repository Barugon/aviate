use crate::{chart_widget::ChartWidget, util};
use godot::{
  engine::{Button, CheckButton, Control, FileDialog, IControl, OptionButton, PanelContainer},
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Control)]
struct MainWidget {
  base: Base<Control>,
}

#[godot_api]
impl MainWidget {
  #[func]
  fn toggle_sidebar(&self, toggle: bool) {
    let this = self.to_gd();
    if let Some(node) = this.find_child("SidebarPanel".into()) {
      let mut sidebar = node.cast::<PanelContainer>();
      sidebar.set_visible(toggle);
    }
  }

  #[func]
  fn open_zip_file(&self) {
    let this = self.to_gd();
    if let Some(node) = this.find_child("FileDialog".into()) {
      let mut file_dialog = node.cast::<FileDialog>();
      if let Some(folder) = dirs::download_dir() {
        if let Some(folder) = folder.to_str() {
          file_dialog.set_current_dir(folder.into());
        }
      }
      file_dialog.show();
    }
  }

  #[func]
  fn zip_file_selected(&self, path: String) {
    if let Ok(info) = util::get_zip_info(&path) {
      match info {
        util::ZipInfo::Chart(files) => {
          let this = self.to_gd();
          if let Some(node) = this.find_child("ChartWidget".into()) {
            let mut chart_widget = node.cast::<ChartWidget>();
            let file = files.first().unwrap().to_str().unwrap().into();
            chart_widget.bind_mut().open_chart(&path, file);
          }
        }
        util::ZipInfo::Aero { csv: _, shp: _ } => (),
      }
    }
  }
}

#[godot_api]
impl IControl for MainWidget {
  fn init(base: Base<Control>) -> Self {
    Self { base }
  }

  fn ready(&mut self) {
    let this = self.to_gd();
    if let Some(node) = this.find_child("SidebarButton".into()) {
      let mut button = node.cast::<CheckButton>();
      button.connect("toggled".into(), this.callable("toggle_sidebar"));
    }

    if let Some(node) = this.find_child("OpenButton".into()) {
      let mut button = node.cast::<Button>();
      button.connect("pressed".into(), this.callable("open_zip_file"));
    }

    if let Some(node) = this.find_child("FileDialog".into()) {
      let mut file_dialog = node.cast::<FileDialog>();
      file_dialog.connect("file_selected".into(), this.callable("zip_file_selected"));

      let vbox = file_dialog.get_vbox().unwrap();
      hide_folders_button(vbox.upcast());
    }
  }
}

/// Iterate until we find the `OptionButton` of folders then hide it.
fn hide_folders_button(node: Gd<Node>) {
  let children = node.get_children();
  for child in children.iter_shared() {
    let name = child.get_name().to_string();
    if name.contains("Container") {
      hide_folders_button(child);
    } else if name.contains("OptionButton") {
      let mut option_button = child.cast::<OptionButton>();
      option_button.set_visible(false);
    }
  }
}
