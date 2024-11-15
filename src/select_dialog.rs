use godot::{
  engine::{Button, ButtonGroup, IWindow, InputEvent, InputEventKey, VBoxContainer, Window},
  global::Key,
  obj::Base,
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Window)]
pub struct SelectDialog {
  base: Base<Window>,
}

#[godot_api]
impl SelectDialog {
  #[signal]
  fn selected(choice: u32);

  #[func]
  fn choice_selected(&mut self) {
    if let Some(node) = self.base().find_child("Items".into()) {
      let items = node.cast::<VBoxContainer>();
      for (idx, node) in items.get_children().iter_shared().enumerate() {
        let button = node.cast::<Button>();
        if button.is_pressed() {
          let mut this = self.base_mut();
          this.hide();
          this.emit_signal("selected".into(), &[Variant::from(idx as u32)]);
        }
      }
    }
  }

  pub fn show_choices<'a, I: Iterator<Item = &'a str>>(&mut self, choices: I) {
    if let Some(node) = self.base().find_child("Items".into()) {
      let mut items = node.cast::<VBoxContainer>();

      // Remove any existing buttons.
      for child in items.get_children().iter_shared() {
        items.remove_child(child.clone());

        // Once removed from the tree, the node must be manually freed.
        child.free();
      }

      // Populate with new buttons.
      let this = self.base();
      let group = ButtonGroup::new_gd();
      for choice in choices {
        let mut button = Button::new_alloc();
        button.set_text(choice.into());
        button.set_toggle_mode(true);
        button.set_button_group(group.clone());
        button.connect("pressed".into(), this.callable("choice_selected"));
        items.add_child(button.upcast());
      }
    }

    // Update the size and show.
    let mut this = self.base_mut();
    this.reset_size();
    this.show();
  }
}

#[godot_api]
impl IWindow for SelectDialog {
  fn init(base: Base<Window>) -> Self {
    Self { base }
  }

  fn ready(&mut self) {
    let mut this = self.base_mut();

    // Make the title font size a bit bigger.
    let property = "theme_override_font_sizes/title_font_size".into();
    this.set(property, Variant::from(16.0));

    // Connect the X button.
    let callable = this.callable("hide");
    this.connect("close_requested".into(), callable.clone());

    // Connect the cancel button.
    if let Some(mut node) = this.find_child("CancelButton".into()) {
      node.connect("pressed".into(), callable);
    }
  }

  fn shortcut_input(&mut self, event: Gd<InputEvent>) {
    let event_key = event.cast::<InputEventKey>();
    if event_key.get_keycode_with_modifiers() == Key::ESCAPE {
      self.base_mut().hide();
    }
  }
}
