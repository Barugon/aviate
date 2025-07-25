use crate::{geom, ok, util};
use godot::{classes::Json, prelude::*};
use std::{cell, rc};

/// Storage for configuration items, persisted as JSON.
pub struct Storage {
  items: rc::Rc<cell::RefCell<Items>>,
}

impl Storage {
  pub fn new() -> Self {
    let items = rc::Rc::new(cell::RefCell::new(Items::load(Storage::path())));
    Self { items }
  }

  pub fn set_win_info(&mut self, win_info: &util::WinInfo) {
    let value = win_info.to_variant();
    self.set_value(Storage::WIN_INFO_KEY, value);
  }

  pub fn get_win_info(&self) -> util::WinInfo {
    util::WinInfo::from_variant(self.get_value(Storage::WIN_INFO_KEY))
  }

  pub fn set_night_mode(&mut self, night_mode: bool) {
    let value = Variant::from(night_mode);
    self.set_value(Storage::NIGHT_MODE_KEY, value);
  }

  pub fn get_night_mode(&self) -> Option<bool> {
    let val = self.get_value(Storage::NIGHT_MODE_KEY)?;
    ok!(val.try_to::<bool>())
  }

  pub fn set_show_bounds(&mut self, bounds: bool) {
    let value = Variant::from(bounds);
    self.set_value(Storage::SHOW_BOUNDS_KEY, value);
  }

  pub fn get_show_bounds(&self) -> Option<bool> {
    let val = self.get_value(Storage::SHOW_BOUNDS_KEY)?;
    ok!(val.try_to::<bool>())
  }

  pub fn set_asset_folder(&mut self, folder: GString) {
    let value = Variant::from(folder);
    self.set_value(Storage::ASSET_FOLDER_KEY, value);
  }

  pub fn get_asset_folder(&self) -> Option<GString> {
    let val = self.get_value(Storage::ASSET_FOLDER_KEY)?;
    ok!(val.try_to::<GString>())
  }

  fn set_value(&mut self, key: &str, val: Variant) {
    self.items.borrow_mut().set(key, val);
  }

  fn get_value(&self, key: &str) -> Option<Variant> {
    self.items.borrow().get(key)
  }

  fn path() -> GString {
    format!("user://{}.json", util::APP_NAME).into()
  }

  const WIN_INFO_KEY: &'static str = "win_info";
  const NIGHT_MODE_KEY: &'static str = "night_mode";
  const SHOW_BOUNDS_KEY: &'static str = "show_bounds";
  const ASSET_FOLDER_KEY: &'static str = "asset_folder";
}

struct Items {
  path: GString,
  items: Dictionary,
}

impl Items {
  fn load(path: GString) -> Self {
    #[cfg(feature = "dev")]
    convert_bounds_svgs();

    let items = Self::load_items(&path);
    Self { path, items }
  }

  fn get(&self, key: &str) -> Option<Variant> {
    self.items.get(key)
  }

  fn set(&mut self, key: &str, item: Variant) {
    let existing = self.items.get_or_nil(key);
    if Json::stringify(&existing) == Json::stringify(&item) {
      return;
    }

    self.items.set(key, item);
    self.store();
  }

  fn store(&self) {
    let text = Json::stringify(&Variant::from(self.items.clone()));
    util::store_text(&self.path, &text);
  }

  fn load_items(path: &GString) -> Dictionary {
    let Some(text) = util::load_text(path) else {
      return Dictionary::new();
    };

    let var = Json::parse_string(&text);
    let Some(items) = ok!(var.try_to::<Dictionary>().map_err(|err| format!("{path:?}: {err}"))) else {
      return Dictionary::new();
    };

    items
  }
}

// Get the bounds for the specified chart in pixel coordinates.
pub fn get_chart_bounds(chart_name: &str, chart_size: geom::Size) -> Vec<geom::Px> {
  let limit = geom::Coord::new((chart_size.w - 1) as f64, (chart_size.h - 1) as f64);
  if let Some(points) = get_bounds_from_json(chart_name, limit) {
    points
  } else {
    // Chart bounds not in the JSON, use the chart size.
    vec![
      geom::Px::new(0.0, 0.0),
      geom::Px::new(limit.x, 0.0),
      geom::Px::new(limit.x, limit.y),
      geom::Px::new(0.0, limit.y),
    ]
  }
}

fn get_bounds_from_json(chart_name: &str, limit: geom::Coord) -> Option<Vec<geom::Px>> {
  // Parse the bounds JSON.
  let json = Json::parse_string(include_str!("../res/bounds.json"));
  let dict = ok!(json.try_to::<Dictionary>())?;

  // Find the chart.
  let array = ok!(dict.get(chart_name)?.try_to::<Array<Variant>>())?;

  // Collect the points.
  let mut points = Vec::with_capacity(array.len());
  for variant in array.iter_shared() {
    let coord = geom::Coord::from_variant(variant)?;
    let x = coord.x.clamp(0.0, limit.x);
    let y = coord.y.clamp(0.0, limit.y);
    points.push(geom::Px::new(x, y));
  }

  Some(points)
}

/// Processes SVG files in '~/Downloads/bounds' and store into 'res/bounds.json'.
#[cfg(feature = "dev")]
fn convert_bounds_svgs() {
  use std::path;

  // Load the bounds JSON file.
  let path = path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res/bounds.json");
  let path = path.to_str().unwrap().into();
  let Some(text) = util::load_text(&path) else {
    return;
  };

  // Parse the JSON.
  let variant = Json::parse_string(&text);
  let Some(mut dict) = ok!(variant.try_to::<Dictionary>()) else {
    return;
  };

  // Get the files from the bounds folder.
  let folder = path::PathBuf::from(util::get_downloads_folder().to_string()).join("bounds");
  let Some(files) = ok!(std::fs::read_dir(&folder)) else {
    return;
  };

  let mut changed = false;
  for entry in files {
    let Some(entry) = ok!(entry) else {
      continue;
    };

    let path = entry.path();
    let Some(ext) = path.extension() else {
      continue;
    };

    if !ext.eq_ignore_ascii_case("svg") {
      continue;
    }

    // Load the svg file.
    let Some(text) = util::load_text(&path.to_str().unwrap().into()) else {
      continue;
    };

    // Find the start of the points text.
    let text = text.to_string();
    let tag = "d=\"M ";
    let Some(pos) = text.find(tag) else {
      continue;
    };

    // Find the end of the points text.
    let text = &text[pos + tag.len()..];
    let tag = "\" />";
    let Some(pos) = text.find(tag) else {
      continue;
    };

    let Some(name) = util::stem_str(&path) else {
      continue;
    };

    let text = &text[..pos];
    let mut first = Default::default();
    let mut prev = Default::default();
    let mut array = Array::new();

    // Create a JSON array from the points.
    for (idx, item) in text.split_ascii_whitespace().enumerate() {
      // Ignore duplicates.
      if item == first || item == prev {
        continue;
      }

      // Get the X and Y values.
      let mut iter = item.split(',');
      let Some(x) = iter.next().and_then(|txt| ok!(txt.parse::<f64>())) else {
        continue;
      };

      let Some(y) = iter.next().and_then(|txt| ok!(txt.parse::<f64>())) else {
        continue;
      };

      prev = item;
      if idx == 0 {
        first = item;
      }

      array.push(&Variant::from([x, y]));
    }

    // Set the new entry.
    dict.set(GString::from(name), Variant::from(array));
    changed = true;

    godot_print!("Added \"{}\" bounds", name);
  }

  if changed {
    // Store the bounds JSON file.
    util::store_text(&path, &Json::stringify(&variant));
  }
}
