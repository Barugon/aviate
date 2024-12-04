use crate::{geom, util};
use godot::{
  classes::{ConfigFile, Json},
  prelude::*,
};
use std::sync;

/// Storage for configuration items, persisted as JSON.
#[derive(Clone)]
pub struct Storage {
  items: sync::Arc<sync::RwLock<Gd<ConfigFile>>>,
}

impl Storage {
  pub fn new() -> Self {
    let mut items = ConfigFile::new_gd();
    items.load(&Storage::path());

    let items = sync::Arc::new(sync::RwLock::new(items));
    Self { items }
  }

  pub fn set_win_info(&mut self, win_info: &util::WinInfo) {
    let value = win_info.to_variant();
    self.set_value(Storage::WIN_INFO_KEY, &value);
  }

  #[allow(unused)]
  pub fn get_win_info(&self) -> util::WinInfo {
    util::WinInfo::from_variant(self.get_value(Storage::WIN_INFO_KEY))
  }

  pub fn set_night_mode(&mut self, dark: bool) {
    let value = Variant::from(dark);
    self.set_value(Storage::NIGHT_MODE_KEY, &value);
  }

  pub fn get_night_mode(&self) -> Option<bool> {
    let val = self.get_value(Storage::NIGHT_MODE_KEY)?;
    val.try_to::<bool>().ok()
  }

  pub fn set_show_bounds(&mut self, bounds: bool) {
    let value = Variant::from(bounds);
    self.set_value(Storage::SHOW_BOUNDS_KEY, &value);
  }

  pub fn get_show_bounds(&self) -> Option<bool> {
    let val = self.get_value(Storage::SHOW_BOUNDS_KEY)?;
    val.try_to::<bool>().ok()
  }

  pub fn set_asset_folder(&mut self, folder: GString) {
    let value = Variant::from(folder);
    self.set_value(Storage::ASSET_FOLDER_KEY, &value);
  }

  pub fn get_asset_folder(&self) -> Option<GString> {
    let val = self.get_value(Storage::ASSET_FOLDER_KEY)?;
    val.try_to::<GString>().ok()
  }

  fn set_value(&mut self, key: &str, val: &Variant) {
    if let Ok(mut items) = self.items.write() {
      items.set_value(Storage::CONFIG_SECTION, key, val);
    }
  }

  fn get_value(&self, key: &str) -> Option<Variant> {
    if let Ok(items) = self.items.read() {
      if items.has_section_key(Storage::CONFIG_SECTION, key) {
        return Some(items.get_value(Storage::CONFIG_SECTION, key));
      }
    }
    None
  }

  fn path() -> String {
    format!("user://{}.cfg", util::APP_NAME)
  }

  const CONFIG_SECTION: &'static str = "config";
  const WIN_INFO_KEY: &'static str = "win_info";
  const NIGHT_MODE_KEY: &'static str = "night_mode";
  const SHOW_BOUNDS_KEY: &'static str = "show_bounds";
  const ASSET_FOLDER_KEY: &'static str = "asset_folder";
}

impl Drop for Storage {
  fn drop(&mut self) {
    if let Ok(mut items) = self.items.write() {
      items.save(&Storage::path());
    }
  }
}

// Get the bounds for the specified chart in pixel coordinates.
pub fn get_chart_bounds(chart_name: &str, chart_size: geom::Size) -> Vec<geom::Coord> {
  let limit = geom::Coord {
    x: (chart_size.w - 1) as f64,
    y: (chart_size.h - 1) as f64,
  };

  if let Some(points) = get_bounds_from_json(chart_name, limit) {
    points
  } else {
    // Chart bounds not in the JSON, use the chart size.
    vec![
      (0.0, 0.0).into(),
      (limit.x, 0.0).into(),
      (limit.x, limit.y).into(),
      (0.0, limit.y).into(),
    ]
  }
}

fn get_bounds_from_json(chart_name: &str, limit: geom::Coord) -> Option<Vec<geom::Coord>> {
  // Parse the bounds JSON.
  let json = Json::parse_string(include_str!("../res/bounds.json"));
  let dict = json.try_to::<Dictionary>().ok()?;

  // Find the chart.
  let array = dict.get(chart_name)?.try_to::<Array<Variant>>().ok()?;

  // Collect the points.
  let mut points = Vec::with_capacity(array.len());
  for variant in array.iter_shared() {
    let coord = geom::Coord::from_variant(variant)?;
    let x = coord.x.max(0.0).min(limit.x);
    let y = coord.y.max(0.0).min(limit.y);
    points.push((x, y).into());
  }

  Some(points)
}

#[cfg(feature = "dev")]
/// Processes SVG files in '~/Downloads/bounds' and store into 'res/bounds.json'.
fn convert_bounds_svgs() {
  let folder = path::PathBuf::from(util::get_downloads_folder().to_string()).join("bounds");
  let Ok(files) = std::fs::read_dir(&folder) else {
    return;
  };

  for entry in files {
    let Ok(entry) = entry else {
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
    let Some(text) = util::load_text(&path) else {
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
    let mut result = String::new();

    // Create a JSON array from the points.
    result += "[[";
    for (idx, item) in text.split_ascii_whitespace().enumerate() {
      // Ignore duplicates.
      if item == first || item == prev {
        continue;
      }

      // Make sure the item is a pair of comma separated values.
      let mut iter = item.split(',');
      let val = iter.next().and_then(|txt| txt.parse::<f64>().ok());
      if val.is_none() {
        continue;
      };

      let val = iter.next().and_then(|txt| txt.parse::<f64>().ok());
      if val.is_none() {
        continue;
      };

      if idx > 0 {
        result += "],[";
      } else {
        first = item;
      }

      result += item;
      prev = item;
    }
    result += "]]";

    // Parse the array.
    let variant = Json::parse_string(&result);
    let Ok(array) = variant.try_to::<Array<Variant>>() else {
      return;
    };

    // Load the bounds JSON file.
    let path = path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res/bounds.json");
    let Some(text) = util::load_text(&path) else {
      return;
    };

    // Parse the JSON.
    let variant = Json::parse_string(&text);
    let Ok(mut dict) = variant.try_to::<Dictionary>() else {
      return;
    };

    // Set the new entry.
    dict.set(GString::from(name), Variant::from(array));

    // Store the bounds JSON file.
    util::store_text(&path, &Json::stringify(&variant));

    godot_print!("Added \"{}\" bounds", name);
  }
}
