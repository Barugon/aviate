use godot::{
  engine::{Button, CheckButton, Control, FileDialog, IControl, PanelContainer},
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
      file_dialog.show();
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
  }
}
