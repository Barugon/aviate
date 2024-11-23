#![allow(unused)]
use crate::util;
use godot::{
  builtin::Variant,
  classes::{os::SystemDir, Os},
  global::godot_error,
};
use std::{borrow::BorrowMut, path, sync};

/// Storage for configuration items, persisted as JSON.
#[derive(Clone)]
pub struct Storage {
  items: sync::Arc<sync::RwLock<inner::Items>>,
  store_win: bool,
}

impl Storage {
  pub fn new(store_win: bool) -> Self {
    let items = sync::Arc::new(sync::RwLock::new(inner::Items::load(Storage::path())));
    Self { items, store_win }
  }

  pub fn set_win_info(&mut self, win_info: &util::WinInfo) {
    if self.store_win {
      let value = win_info.to_variant();
      let mut items = self.items.write().unwrap();
      items.set(Storage::WIN_INFO_KEY, value);
      items.store_items();
    }
  }

  pub fn get_win_info(&self) -> util::WinInfo {
    let items = self.items.read().unwrap();
    util::WinInfo::from_variant(items.get(Storage::WIN_INFO_KEY))
  }

  pub fn set_night_mode(&mut self, dark: bool) {
    let value = Variant::from(dark);
    let mut items = self.items.write().unwrap();
    items.set(Storage::NIGHT_MODE_KEY, value);
    items.store_items();
  }

  pub fn get_night_mode(&self) -> Option<bool> {
    let items = self.items.read().unwrap();
    match items.get(Storage::NIGHT_MODE_KEY)?.try_to::<bool>() {
      Ok(value) => return Some(value),
      Err(err) => godot_error!("{err}"),
    }
    None
  }

  pub fn set_asset_folder(&mut self, folder: String) {
    let value = Variant::from(folder);
    let mut items = self.items.write().unwrap();
    items.set(Storage::ASSET_FOLDER_KEY, value);
    items.store_items();
  }

  pub fn get_asset_folder(&self) -> Option<String> {
    let items = self.items.read().unwrap();
    match items.get(Storage::ASSET_FOLDER_KEY)?.try_to::<String>() {
      Ok(value) => return Some(value),
      Err(err) => godot_error!("{err}"),
    }
    None
  }

  fn path() -> path::PathBuf {
    let folder = path::PathBuf::from(util::get_config_folder());
    folder.join(util::APP_NAME).with_extension("json")
  }

  const WIN_INFO_KEY: &'static str = "win_info";
  const NIGHT_MODE_KEY: &'static str = "night_mode";
  const ASSET_FOLDER_KEY: &'static str = "asset_folder";
}

mod inner {
  use godot::{
    builtin::{Dictionary, Variant},
    classes::{json, Json},
    global::{godot_error, godot_print, Error},
    obj::{EngineEnum, Gd, NewGd},
  };
  use std::{
    fs, io, path,
    sync::{self, atomic, mpsc},
    thread,
  };

  pub struct Items {
    path: path::PathBuf,
    items: Dictionary,
    changed: atomic::AtomicBool,
  }

  impl Items {
    pub fn load(path: path::PathBuf) -> Self {
      let items = Self::load_items(&path);
      let changed = atomic::AtomicBool::new(false);
      Self {
        path,
        items,
        changed,
      }
    }

    pub fn get(&self, key: &str) -> Option<Variant> {
      self.items.get(key)
    }

    pub fn set(&mut self, key: &str, item: Variant) {
      if self.items.get_or_nil(key) == item {
        return;
      }
      self.items.set(key, item);
      self.changed.store(true, atomic::Ordering::Relaxed);
    }

    fn load_items(path: &path::Path) -> Dictionary {
      if let Ok(text) = fs::read_to_string(path) {
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

    pub fn store_items(&self) {
      if self.changed.swap(false, atomic::Ordering::Relaxed) {
        let text = Json::stringify(&Variant::from(self.items.clone()));
        match fs::write(&self.path, text.to_string()) {
          Ok(()) => (),
          Err(err) => {
            godot_error!("{:?}: {}", self.path, err);
          }
        }
      }
    }
  }

  impl Drop for Items {
    fn drop(&mut self) {
      self.store_items();
    }
  }
}
