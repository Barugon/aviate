mod chart;
mod chart_widget;
mod config;
mod find_dialog;
mod geom;
mod main_widget;
mod nasr;
mod select_dialog;
mod util;

use godot::prelude::*;

struct AviateExtension;

#[gdextension]
unsafe impl ExtensionLibrary for AviateExtension {}
