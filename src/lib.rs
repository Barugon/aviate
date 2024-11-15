mod chart;
mod chart_widget;
mod main_widget;
mod select_dialog;
mod util;

use godot::prelude::*;

struct AviateExtension;

#[gdextension]
unsafe impl ExtensionLibrary for AviateExtension {}
