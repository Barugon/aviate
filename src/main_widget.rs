use crate::{chart_widget::ChartWidget, util};
use godot::{
  engine::{
    AcceptDialog, Button, CheckButton, Control, FileDialog, HBoxContainer, IControl, PanelContainer,
  },
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
    let this = self.to_gd();

    // The file dialog needs to be hidden first or it will generate an error if the alert dialog is shown.
    if let Some(node) = this.find_child("FileDialog".into()) {
      let mut file_dialog = node.cast::<FileDialog>();
      file_dialog.hide();
    }

    match util::get_zip_info(&path) {
      Ok(info) => match info {
        util::ZipInfo::Chart(files) => {
          if files.len() == 1 {
            if let Some(node) = this.find_child("ChartWidget".into()) {
              let mut chart_widget = node.cast::<ChartWidget>();
              let mut chart_widget = chart_widget.bind_mut();
              let file = files.first().unwrap().to_str().unwrap().into();
              if let Err(err) = chart_widget.open_chart(&path, file) {
                self.show_alert(err.as_ref());
              }
            }
          } else {
            self.show_alert("Multi-file charts are not yet supported");
          }
        }
        util::ZipInfo::Aero { csv: _, shp: _ } => {
          self.show_alert("Aero data is not yet supported");
        }
      },
      Err(err) => {
        self.show_alert(err.as_ref());
      }
    }
  }

  fn show_alert(&self, text: &str) {
    let this = self.to_gd();
    if let Some(child) = this.find_child("AlertDialog".into()) {
      let mut alert_dialog = child.cast::<AcceptDialog>();
      alert_dialog.set_text(text.into());
      alert_dialog.reset_size();
      alert_dialog.show();
    } else {
      godot_error!("{text}");
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
      hide_buttons(vbox.upcast());
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
