use godot::{
  classes::{Button, Control, IWindow, InputEvent, InputEventKey, Tree, Window},
  global::{Key, KeyModifierMask},
  prelude::*,
};

#[derive(GodotClass)]
#[class(base=Window)]
pub struct SelectDialog {
  base: Base<Window>,
  tree: OnReady<Gd<Tree>>,
}

#[godot_api]
impl SelectDialog {
  #[signal]
  fn selected(choice: u32);

  #[func]
  fn choice_confirmed(&mut self) {
    if let Some(mut item) = self.tree.get_selected() {
      let idx = item.get_index();
      let mut this = self.base_mut();
      this.hide();
      this.emit_signal("selected", &[Variant::from(idx as u32)]);
    }
  }

  #[func]
  fn choice_selected(&self) {
    let mut button = self.get_child::<Button>("OkButton");
    button.set_disabled(false);
  }

  pub fn show_choices<'a, I: Iterator<Item = &'a str>>(&mut self, choices: I) {
    // Disable the ok button.
    let mut button = self.get_child::<Button>("OkButton");
    button.set_disabled(true);

    // Remove existing choices.
    self.tree.clear();
    self.tree.set_column_expand_ratio(0, 2);
    self.tree.set_column_expand(0, true);

    // Populate with new choices.
    let root = self.tree.create_item().unwrap();
    for choice in choices {
      let mut item = self.tree.create_item_ex().parent(&root).done().unwrap();
      item.set_expand_right(0, true);
      if let Some(pos) = choice.find('(') {
        let (name, info) = choice.split_at(pos);
        item.set_text(0, name);
        item.set_text(1, info);
      } else {
        item.set_text(0, choice);
      }
    }

    self.tree.scroll_to_item(&root);

    // Adjust the width if it's greater than the parent.
    const DECO_WIDTH: i32 = 16;
    let parent = self.base().get_parent().unwrap();
    let parent = parent.cast::<Control>();
    let size = self.base().get_size();
    let parent_width = parent.get_size().x as i32;
    if size.x + DECO_WIDTH > parent_width {
      let new_size = Vector2i::new(parent_width - DECO_WIDTH, size.y);
      self.base_mut().set_size(new_size);
    }

    self.base_mut().call_deferred("show", &[]);
  }

  fn get_child<T: Inherits<Node>>(&self, name: &str) -> Gd<T> {
    self.base().find_child(name).unwrap().cast()
  }
}

#[godot_api]
impl IWindow for SelectDialog {
  fn init(base: Base<Window>) -> Self {
    Self {
      base,
      tree: OnReady::manual(),
    }
  }

  fn ready(&mut self) {
    // Get the items vbox.
    self.tree.init(self.get_child("Tree"));

    let callable = self.base().callable("choice_confirmed");
    self.tree.connect("item_activated", &callable);

    let callable = self.base().callable("choice_selected");
    self.tree.connect("item_selected", &callable);

    // Make the title font size a bit bigger.
    let property = "theme_override_font_sizes/title_font_size";
    self.base_mut().set(property, &Variant::from(16.0));

    // Connect the X button.
    let callable = self.base().callable("hide");
    self.base_mut().connect("close_requested", &callable);

    // Connect the cancel button.
    let mut button = self.get_child::<Button>("CancelButton");
    button.connect("pressed", &callable);

    // Connect the cancel button.
    let callable = self.base().callable("choice_confirmed");
    let mut button = self.get_child::<Button>("OkButton");
    button.connect("pressed", &callable);
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
