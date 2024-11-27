use crate::{geom, util};
use godot::{classes::Json, prelude::*};
use std::{cell, path, rc, sync::atomic};

/// Storage for configuration items, persisted as JSON.
#[derive(Clone)]
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
    let mut items = (*self.items).borrow_mut();
    items.set(Storage::WIN_INFO_KEY, value);
    items.store();
  }

  #[allow(unused)]
  pub fn get_win_info(&self) -> util::WinInfo {
    let items = (*self.items).borrow();
    util::WinInfo::from_variant(items.get(Storage::WIN_INFO_KEY))
  }

  pub fn set_night_mode(&mut self, dark: bool) {
    let value = Variant::from(dark);
    let mut items = (*self.items).borrow_mut();
    items.set(Storage::NIGHT_MODE_KEY, value);
    items.store();
  }

  pub fn get_night_mode(&self) -> Option<bool> {
    let items = (*self.items).borrow();
    match items.get(Storage::NIGHT_MODE_KEY)?.try_to::<bool>() {
      Ok(value) => return Some(value),
      Err(err) => godot_error!("{err}"),
    }
    None
  }

  pub fn set_asset_folder(&mut self, folder: GString) {
    let value = Variant::from(folder);
    let mut items = (*self.items).borrow_mut();
    items.set(Storage::ASSET_FOLDER_KEY, value);
    items.store();
  }

  pub fn get_asset_folder(&self) -> Option<GString> {
    let items = (*self.items).borrow();
    match items.get(Storage::ASSET_FOLDER_KEY)?.try_to::<GString>() {
      Ok(value) => return Some(value),
      Err(err) => godot_error!("{err}"),
    }
    None
  }

  fn path() -> path::PathBuf {
    let folder = util::get_config_folder().to_string();
    let folder = path::PathBuf::from(folder);
    folder.join(util::APP_NAME).with_extension("json")
  }

  const WIN_INFO_KEY: &'static str = "win_info";
  const NIGHT_MODE_KEY: &'static str = "night_mode";
  const ASSET_FOLDER_KEY: &'static str = "asset_folder";
}

struct Items {
  path: path::PathBuf,
  items: Dictionary,
  changed: atomic::AtomicBool,
}

impl Items {
  fn load(path: path::PathBuf) -> Self {
    #[cfg(feature = "dev")]
    perform_housekeeping();

    let items = Self::load_items(&path);
    let changed = atomic::AtomicBool::new(false);
    Self {
      path,
      items,
      changed,
    }
  }

  fn get(&self, key: &str) -> Option<Variant> {
    self.items.get(key)
  }

  fn set(&mut self, key: &str, item: Variant) {
    let existing = self.items.get_or_nil(key);
    if item.try_to::<Dictionary>().is_ok() {
      if Json::stringify(&existing) == Json::stringify(&item) {
        return;
      }
    } else if existing == item {
      return;
    }

    self.items.set(key, item);
    self.changed.store(true, atomic::Ordering::Relaxed);
  }

  fn store(&self) {
    if self.changed.swap(false, atomic::Ordering::Relaxed) {
      let text = Json::stringify(&Variant::from(self.items.clone()));
      util::store_text(&self.path, &text);
    }
  }

  fn load_items(path: &path::Path) -> Dictionary {
    if let Some(text) = util::load_text(path) {
      let items = Json::parse_string(&text);
      match items.try_to::<Dictionary>() {
        Ok(items) => {
          return items;
        }
        Err(err) => {
          godot_error!("{:?}: {}", path, err);
        }
      }
    }
    Dictionary::new()
  }
}

impl Drop for Items {
  fn drop(&mut self) {
    self.store();
  }
}

const BOUNDS_JSON: &str = include_str!("../res/bounds.json");

// Get the bounds for the specified chart in pixel coordinates.
pub fn get_chart_bounds(chart_name: &str) -> Option<Vec<geom::Coord>> {
  // Parse the bounds JSON.
  let json = Json::parse_string(BOUNDS_JSON);
  let dict = json.try_to::<Dictionary>().ok()?;

  // Find the chart.
  let array = dict.get(chart_name)?.try_to::<Array<Variant>>().ok()?;

  // Collect the points.
  let mut points = Vec::with_capacity(array.len());
  for variant in array.iter_shared() {
    let coord = geom::Coord::from_variant(variant)?;
    points.push((coord.x, coord.y).into());
  }

  Some(points)
}

#[cfg(feature = "dev")]
fn perform_housekeeping() {
  compact_bounds_json();
  convert_path();
}

#[cfg(feature = "dev")]
fn convert_path() {
  let path = path::PathBuf::from(util::get_downloads_folder().to_string()).join("path");
  let Some(text) = util::load_text(&path) else {
    return;
  };

  let text = text.to_string();
  let Some(pos) = text.find("d=\"M ") else {
    return;
  };

  let text = &text[pos + 5..];
  let Some(pos) = text.find(" C ") else {
    return;
  };

  let text = &text[pos + 3..];
  let Some(pos) = text.find("\" />") else {
    return;
  };

  let text = &text[..pos];
  let mut prev = Default::default();
  let mut result = String::new();

  result += "[[";
  for (idx, item) in text.split_ascii_whitespace().enumerate() {
    if item == prev {
      continue;
    }

    if idx > 0 {
      result += "],[";
    }
    result += item;
    prev = item;
  }
  result += "]]";

  util::store_text(&path, &GString::from(result));
}

#[cfg(feature = "dev")]
fn compact_bounds_json() {
  let text = Json::stringify(&Json::parse_string(BOUNDS_JSON));
  let path = path::Path::new(env!("CARGO_MANIFEST_DIR")).join("res/bounds.json");
  util::store_text(&path, &text);
}
