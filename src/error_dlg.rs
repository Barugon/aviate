use crate::util;
use eframe::{egui, emath, epaint};
use std::mem;

#[derive(Default)]
pub struct ErrorDlg {
  text: Option<util::Error>,
  reset: bool,
}

impl ErrorDlg {
  pub fn open(text: util::Error) -> Self {
    Self {
      text: Some(text),
      reset: true,
    }
  }

  pub fn show(&mut self, ctx: &egui::Context) -> bool {
    if ctx
      .input(|state| state.key_pressed(egui::Key::Enter) || state.key_pressed(egui::Key::Escape))
    {
      self.text = None;
    }

    let mut open = self.text.is_some();
    let win = egui::Window::new(egui::RichText::from("⚠  Error").strong())
      .open(&mut open)
      .collapsible(false)
      .resizable(false)
      .anchor(emath::Align2::CENTER_CENTER, [0.0, 0.0]);

    // Hack to reset the window size.
    let win = if mem::take(&mut self.reset) {
      win.fixed_size([200.0, 20.0])
    } else {
      win
    };

    win.show(ctx, |ui| {
      ui.add_space(8.0);
      ui.vertical_centered(|ui| {
        if let Some(text) = self.text.as_ref() {
          let text = egui::RichText::from(text.as_ref());
          let widget = egui::Label::new(text.color(epaint::Color32::LIGHT_RED))
            .wrap_mode(egui::TextWrapMode::Extend);
          ui.add(widget);
        }
      });
      ui.add_space(8.0);
      ui.separator();
      ui.horizontal(|ui| {
        if ui.button("Close").clicked() {
          self.text = None;
        }
      });
    });

    if self.text.is_none() && open {
      open = false;
    }

    open
  }
}
