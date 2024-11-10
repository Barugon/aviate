use godot::{
  engine::{CheckButton, Control, IControl, PanelContainer},
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
  }
}
