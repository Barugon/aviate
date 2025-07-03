use crate::{geom, util};
use godot::{
  classes::{Button, IWindow, InputEvent, RichTextLabel, Window},
  global::{Key, KeyModifierMask},
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Window)]
pub struct InfoDialog {
  base: Base<Window>,
  text: OnReady<Gd<RichTextLabel>>,
  coord: geom::DD,
}

#[godot_api]
impl InfoDialog {
  #[signal]
  fn confirmed(var: Variant);

  #[func]
  fn confirm(&mut self) {
    let coord = self.coord;
    self.base_mut().hide();
    self.base_mut().emit_signal("confirmed", vslice![coord]);
  }

  pub fn show_info(&mut self, text: &str, coord: geom::DD) {
    self.coord = coord;
    self.text.set_text(text);
    self.text.scroll_to_line(0);
    self.base_mut().call_deferred("show", &[]);
  }

  fn get_child<T: Inherits<Node>>(&self, name: &str) -> Gd<T> {
    self.base().find_child(name).unwrap().cast()
  }
}

#[godot_api]
impl IWindow for InfoDialog {
  fn init(base: Base<Window>) -> Self {
    Self {
      base,
      text: OnReady::manual(),
      coord: Default::default(),
    }
  }

  fn ready(&mut self) {
    // Initialize the rich text label.
    self.text.init(self.get_child("RichTextLabel"));

    // Setup the Go To Button.
    let callable = self.base().callable("confirm");
    let mut child = self.get_child::<Button>("GoToButton");
    child.connect("pressed", &callable);

    // Connect the X button.
    let callable = self.base().callable("hide");
    self.base_mut().connect("close_requested", &callable);

    // Connect the Close button.
    let mut child = self.get_child::<Button>("CloseButton");
    child.connect("pressed", &callable);
  }

  fn shortcut_input(&mut self, event: Gd<InputEvent>) {
    let Ok(key_event) = event.try_cast::<godot::classes::InputEventKey>() else {
      return;
    };

    if key_event.get_keycode() == Key::ESCAPE && key_event.get_modifiers_mask() == KeyModifierMask::default() {
      self.base_mut().hide();
    }
  }

  fn process(&mut self, _: f64) {
    util::adjust_dialog(&mut self.base_mut());
  }
}
