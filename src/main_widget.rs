use crate::{chart_widget::ChartWidget, util};
use godot::{
  engine::{
    AcceptDialog, Button, CheckButton, Control, FileDialog, HBoxContainer, IControl, PanelContainer,
  },
  global::HorizontalAlignment,
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
    if let Some(node) = self.base().find_child("SidebarPanel".into()) {
      let mut sidebar = node.cast::<PanelContainer>();
      sidebar.set_visible(toggle);
    }
  }

  #[func]
  fn open_zip_file(&self) {
    if let Some(node) = self.base().find_child("FileDialog".into()) {
      let mut file_dialog = node.cast::<FileDialog>();
      let property = "theme_override_font_sizes/title_font_size".into();
      file_dialog.set(property, Variant::from(16.0));

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
    let this = self.base();

    // The file dialog needs to be hidden first or it will generate an error if the alert dialog is shown.
    if let Some(node) = this.find_child("FileDialog".into()) {
      let mut file_dialog = node.cast::<FileDialog>();
      file_dialog.hide();
    }

    match util::get_zip_info(&path) {
      Ok(info) => match info {
        util::ZipInfo::Chart(files) => {
          if files.len() > 1 {
            self.show_alert("Multi-file charts are not yet supported");
          } else {
            if let Some(node) = this.find_child("ChartWidget".into()) {
              let mut chart_widget = node.cast::<ChartWidget>();
              let mut chart_widget = chart_widget.bind_mut();
              let file = files.first().and_then(|f| f.to_str()).unwrap();
              if let Err(err) = chart_widget.open_chart(&path, file) {
                self.show_alert(err.as_ref());
              }
            }
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
    if let Some(child) = self.base().find_child("AlertDialog".into()) {
      let mut alert_dialog = child.cast::<AcceptDialog>();
      let property = "theme_override_font_sizes/title_font_size".into();
      alert_dialog.set(property, Variant::from(16.0));

      if let Some(label) = alert_dialog.get_label() {
        let mut label = label;
        let property = "theme_override_colors/font_color".into();
        let color = Variant::from(Color::from_rgb(1.0, 0.4, 0.4));
        label.set(property, color);
        label.set_horizontal_alignment(HorizontalAlignment::CENTER);
      }

      alert_dialog.set_text(text.into());
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
    Self { base }
  }

  fn ready(&mut self) {
    let this = self.base();
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
