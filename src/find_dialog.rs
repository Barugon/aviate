use godot::{
  classes::{
    notify::WindowNotification, Button, IWindow, InputEvent, InputEventKey, LineEdit, Window,
  },
  global::{Key, KeyModifierMask},
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Window)]
pub struct FindDialog {
  base: Base<Window>,
  ok: OnReady<Gd<Button>>,
}

#[godot_api]
impl FindDialog {
  #[signal]
  fn confirmed(text: GString);

  #[func]
  fn changed(&mut self, text: GString) {
    self.ok.set_disabled(text.is_empty());
  }

  #[func]
  fn submit(&mut self, text: GString) {
    if text.is_empty() {
      return;
    }

    let text = Variant::from(text);
    let mut this = self.base_mut();
    this.hide();
    this.emit_signal("confirmed", &[text]);
  }

  #[func]
  fn confirm(&mut self) {
    let text = self.get_child::<LineEdit>("LineEdit").get_text();
    if text.is_empty() {
      return;
    }

    let text = Variant::from(text);
    let mut this = self.base_mut();
    this.hide();
    this.emit_signal("confirmed", &[text]);
  }

  fn get_child<T: Inherits<Node>>(&self, name: &str) -> Gd<T> {
    self.base().find_child(name).unwrap().cast()
  }
}

#[godot_api]
impl IWindow for FindDialog {
  fn init(base: Base<Window>) -> Self {
    Self {
      base,
      ok: OnReady::manual(),
    }
  }

  fn on_notification(&mut self, what: WindowNotification) {
    if what == WindowNotification::VISIBILITY_CHANGED {
      let callable = self.base().callable("changed");
      let mut child = self.get_child::<LineEdit>("LineEdit");
      if self.base().is_visible() {
        child.clear();
        child.grab_focus();
        child.connect("text_changed", &callable);
      } else {
        child.disconnect("text_changed", &callable);
        self.ok.set_disabled(true);
      }
    }
  }

  fn ready(&mut self) {
    // Setup the line edit.
    let mut child = self.get_child::<LineEdit>("LineEdit");
    child.connect("text_submitted", &self.base().callable("submit"));

    // Setup the Ok Button.
    let callable = self.base().callable("confirm");
    self.ok.init(self.get_child("OkButton"));
    self.ok.connect("pressed", &callable);

    // Connect the X button.
    let callable = self.base().callable("hide");
    self.base_mut().connect("close_requested", &callable);

    // Connect the Cancel button.
    let mut child = self.get_child::<Button>("CancelButton");
    child.connect("pressed", &callable);
  }

  fn shortcut_input(&mut self, event: Gd<InputEvent>) {
    let event_key = event.cast::<InputEventKey>();
    if event_key.get_keycode() == Key::ESCAPE
      && event_key.get_modifiers_mask() == KeyModifierMask::default()
    {
      self.base_mut().hide();
    }
  }
}