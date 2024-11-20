use godot::{
  classes::{Button, ButtonGroup, IWindow, InputEvent, InputEventKey, VBoxContainer, Window},
  global::{Key, KeyModifierMask},
  obj::Base,
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Window)]
pub struct SelectDialog {
  base: Base<Window>,
  items: OnReady<Gd<VBoxContainer>>,
}

#[godot_api]
impl SelectDialog {
  #[signal]
  fn selected(choice: u32);

  #[func]
  fn choice_selected(&mut self) {
    for (idx, node) in self.items.get_children().iter_shared().enumerate() {
      let button = node.cast::<Button>();
      if button.is_pressed() {
        let mut this = self.base_mut();
        this.hide();
        this.emit_signal("selected", &[Variant::from(idx as u32)]);
      }
    }
  }

  pub fn show_choices<'a, I: Iterator<Item = &'a str>>(&mut self, choices: I) {
    // Remove any existing buttons.
    for child in self.items.get_children().iter_shared() {
      self.items.remove_child(&child);

      // Once removed from the tree, the node must be manually freed.
      child.free();
    }

    // Populate with new buttons.
    let this = self.base();
    let group = ButtonGroup::new_gd();
    let callable = this.callable("choice_selected");
    for choice in choices {
      let mut button = Button::new_alloc();
      button.set_text(choice);
      button.set_toggle_mode(true);
      button.set_button_group(&group);
      button.connect("pressed", &callable);
      self.items.add_child(&button);
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
    Self {
      base,
      items: OnReady::manual(),
    }
  }

  fn ready(&mut self) {
    // Get the items vbox.
    let node = self.base().find_child("Items").unwrap();
    self.items.init(node.cast());

    let mut this = self.base_mut();

    // Make the title font size a bit bigger.
    let property = "theme_override_font_sizes/title_font_size";
    this.set(property, &Variant::from(16.0));

    // Connect the X button.
    let callable = this.callable("hide");
    this.connect("close_requested", &callable);

    // Connect the cancel button.
    let mut node = this.find_child("CancelButton").unwrap();
    node.connect("pressed", &callable);
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
