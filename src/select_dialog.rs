use crate::util;
use godot::{
  classes::{Button, IWindow, InputEvent, InputEventKey, Tree, Window},
  global::{Key, KeyModifierMask},
  prelude::*,
};
use std::borrow;

#[derive(GodotClass)]
#[class(base=Window)]
pub struct SelectDialog {
  base: Base<Window>,
  tree: OnReady<Gd<Tree>>,
  width: i32,
}

#[godot_api]
impl SelectDialog {
  #[signal]
  fn item_confirmed(choice: u32);

  #[signal]
  fn info_confirmed(choice: u32);

  #[func]
  fn choose_item(&mut self) {
    let Some(mut item) = self.tree.get_selected() else {
      return;
    };

    let idx = item.get_index();
    let mut this = self.base_mut();
    this.hide();
    this.emit_signal("item_confirmed", vslice![idx as u32]);
  }

  #[func]
  fn choose_info(&mut self) {
    let Some(mut item) = self.tree.get_selected() else {
      return;
    };

    let idx = item.get_index();
    let mut this = self.base_mut();
    this.hide();
    this.emit_signal("info_confirmed", vslice![idx as u32]);
  }

  #[func]
  fn choice_selected(&self) {
    self.get_child::<Button>("OkButton").set_disabled(false);
    self.get_child::<Button>("InfoButton").set_disabled(false);
  }

  pub fn show_choices<'a, I: Iterator<Item = Option<borrow::Cow<'a, str>>>>(
    &mut self,
    choices: I,
    title: &str,
    ok_name: &str,
    show_info: bool,
  ) {
    // Remove existing choices and disable scrolling.
    self.tree.clear();

    // Disable scrolling.
    self.tree.set_v_scroll_enabled(false);

    // Set column sizing.
    self.tree.set_column_expand(0, true);
    self.tree.set_column_expand(1, false);

    // Populate with new choices.
    let root = self.tree.create_item().unwrap();
    let count = {
      let mut count = 0;
      let mut set_min = true;
      for choice in choices {
        let Some(choice) = choice else {
          continue;
        };

        let mut item = self.tree.create_item_ex().parent(&root).done().unwrap();
        if let Some(pos) = choice.rfind('(') {
          let (name, info) = choice.split_at(pos);
          let name = GString::from(name.trim());
          item.set_text(0, &name);
          item.set_text(1, info.trim());

          #[cfg(not(target_os = "android"))]
          item.set_tooltip_text(0, &name);

          if set_min {
            self.tree.set_column_custom_minimum_width(1, 100);
            set_min = false;
          }
        } else {
          item.set_text(0, choice.trim());
          if set_min {
            self.tree.set_column_custom_minimum_width(1, 4);
            set_min = false;
          }
        }
        count += 1;
      }
      count
    };

    let mut button = self.get_child::<Button>("OkButton");
    button.set_disabled(true);
    button.set_text(ok_name);

    let mut button = self.get_child::<Button>("InfoButton");
    button.set_disabled(true);
    button.set_visible(show_info);

    self.base_mut().set_title(title);
    self.base_mut().reset_size();

    // Reenable scrolling.
    self.tree.set_v_scroll_enabled(true);
    self.tree.scroll_to_item(&root);

    // Resize the window.
    let size = Vector2i::new(self.width, self.base().get_size().y);
    self.base_mut().set_size(size);

    // Show the window.
    self.base_mut().call_deferred("show", &[]);

    // If there's only one choice then select it.
    if count == 1 {
      let mut root = root;
      if let Some(item) = root.get_child(0) {
        let args = [Variant::from(item), Variant::from(0)];
        self.tree.grab_focus();
        self.tree.call_deferred("set_selected", &args);
      }
    }
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
      width: 0,
    }
  }

  fn ready(&mut self) {
    // Remember the dialog width.
    self.width = self.base().get_size().x;

    // Get the items tree.
    self.tree.init(self.get_child("Tree"));

    // Connect the ok button.
    let callable = self.base().callable("choose_item");
    let mut button = self.get_child::<Button>("OkButton");
    button.connect("pressed", &callable);
    self.tree.connect("item_activated", &callable);

    // Connect the ok button.
    let callable = self.base().callable("choose_info");
    let mut button = self.get_child::<Button>("InfoButton");
    button.connect("pressed", &callable);

    // Connect the selected callback.
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

    #[cfg(target_os = "android")]
    util::hide_hover(&mut self.tree);
  }

  fn shortcut_input(&mut self, event: Gd<InputEvent>) {
    let Ok(key_event) = event.try_cast::<InputEventKey>() else {
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
