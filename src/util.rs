use crate::geom;
use gdal::vector;
use godot::{
  classes::{
    Control, DisplayServer, FileAccess, Os, Window, display_server::WindowMode, file_access::ModeFlags, os::SystemDir,
  },
  prelude::*,
};
use std::{borrow, cmp, collections, ops, path, sync, time};

pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
pub const PROJ4_NAD83: &str = "+proj=longlat +datum=NAD83 +no_defs";
pub const ZOOM_RANGE: ops::RangeInclusive<f32> = 1.0 / 8.0..=1.0;
pub const TITLE_HEIGHT: i32 = 32;
pub const BORDER_WIDTH: i32 = 8;
pub const BORDER_HEIGHT: i32 = 6;
pub const MIN_FIND_CHARS: usize = 3;

/// Convert `Result` into `Option` and print any error.
#[macro_export]
macro_rules! ok {
  ($result:expr) => {
    match $result {
      Ok(ok) => Some(ok),
      Err(err) => {
        godot::global::godot_error!("{err}\n{}:{}:{}", file!(), line!(), column!());
        None
      }
    }
  };
}

pub struct Timer {
  start: time::Instant,
}

#[allow(unused)]
impl Timer {
  pub fn new() -> Self {
    let start = time::Instant::now();
    Self { start }
  }

  pub fn elapsed(&self) -> time::Duration {
    self.start.elapsed()
  }

  /// Print the elapsed time to the console.
  pub fn print(&self) {
    godot_print!("{:?}", self.elapsed());
  }
}

#[derive(Clone, Default)]
pub struct Cancel {
  canceled: sync::Arc<sync::atomic::AtomicBool>,
}

impl Cancel {
  pub fn cancel(&mut self) {
    self.canceled.store(true, sync::atomic::Ordering::Relaxed);
  }

  pub fn canceled(&self) -> bool {
    self.canceled.load(sync::atomic::Ordering::Relaxed)
  }
}

/// Error message as either `&'static str` or `String`.
pub type Error = borrow::Cow<'static, str>;

pub enum ZipInfo {
  /// Chart raster data.
  Chart(Vec<path::PathBuf>),

  /// NASR aeronautical data.
  Aero {
    csv: path::PathBuf,
    #[allow(unused)]
    shp: path::PathBuf,
  },
}

/// Returns information about what type of FAA data (if any) is contained in a zip file.
pub fn get_zip_info(path: &path::Path) -> Result<ZipInfo, Error> {
  /// Return a path if the folder contains CSV data.
  fn get_csv_path(path: &path::Path, folder: &path::Path) -> Option<path::PathBuf> {
    let files = ok!(gdal::vsi::read_dir(path.join(folder), false))?;
    for file in files {
      let Some(ext) = file.extension() else {
        continue;
      };

      if !ext.eq_ignore_ascii_case("zip") {
        continue;
      }

      let Some(stem) = stem_str(&file) else {
        continue;
      };

      if stem.to_ascii_uppercase().ends_with("_CSV") {
        return Some(folder.join(file));
      }
    }
    None
  }

  /// Return a path if the folder contains shape-file data.
  fn get_shp_path(path: &path::Path, folder: &path::Path) -> Option<path::PathBuf> {
    let files = ok!(gdal::vsi::read_dir(path.join(folder), false))?;
    for file in files {
      let Some(name) = file.file_name() else {
        continue;
      };

      if name.eq_ignore_ascii_case("Shape_Files") {
        return get_shp_path(path, &folder.join(name));
      }

      let Some(stem) = path::Path::new(name).file_stem() else {
        continue;
      };

      if stem.eq_ignore_ascii_case("Class_Airspace") {
        // Use the folder for shape files.
        return Some(folder.into());
      }
    }
    None
  }

  const EXT_ERR: Error = borrow::Cow::Borrowed("Path must have 'zip' extension");
  if let Some(ext) = path.extension() {
    if !ext.eq_ignore_ascii_case("zip") {
      return Err(EXT_ERR);
    }
  } else {
    return Err(EXT_ERR);
  }

  let Some(path) = path.to_str() else {
    return Err("Invalid unicode in zip file path".into());
  };

  // Concatenate the VSI prefix.
  let path = ["/vsizip/", path].concat();
  let path = path::PathBuf::from(path);

  let files = match gdal::vsi::read_dir(&path, false) {
    Ok(files) => files,
    Err(_) => return Err("Unable to read zip file".into()),
  };

  let mut csv = path::PathBuf::new();
  let mut shp = path::PathBuf::new();
  let mut tfws = collections::HashSet::new();
  let mut tifs = Vec::new();
  for file in files {
    let Some(name) = file.file_name() else {
      continue;
    };

    // Look for aeronautical data.
    if name.eq_ignore_ascii_case("Additional_Data") {
      if let Some(shp_path) = get_shp_path(&path, path::Path::new(name)) {
        shp = shp_path;
      }
    } else if name.eq_ignore_ascii_case("CSV_Data") {
      if let Some(csv_path) = get_csv_path(&path, path::Path::new(name)) {
        csv = csv_path;
      }
    }

    if !csv.as_os_str().is_empty() && !shp.as_os_str().is_empty() {
      return Ok(ZipInfo::Aero { csv, shp });
    }

    let Some(ext) = file.extension() else {
      continue;
    };

    // Check for chart data.
    if ext.eq_ignore_ascii_case("tfw") {
      if let Some(stem) = file.file_stem() {
        tfws.insert(path::PathBuf::from(stem));
      }
    } else if ext.eq_ignore_ascii_case("tif") {
      tifs.push(file);
    }
  }

  let mut files = Vec::with_capacity(cmp::min(tifs.len(), tfws.len()));
  for file in tifs {
    let Some(stem) = file.file_stem() else {
      continue;
    };

    // Only accept TIFF files that have matching TFW files.
    if tfws.contains(path::Path::new(stem)) {
      files.push(file);
    }
  }

  if files.is_empty() {
    return Err("Zip file does not contain usable data".into());
  }

  Ok(ZipInfo::Chart(files))
}

#[derive(Default, Eq, PartialEq)]
pub struct WinInfo {
  pub pos: Option<geom::Pos>,
  pub size: Option<geom::Size>,
  pub maxed: bool,
}

impl WinInfo {
  pub fn from_display(display_server: &Gd<DisplayServer>) -> Self {
    let pos = Some(display_server.window_get_position().into());
    let size = Some(display_server.window_get_size().into());
    let maxed = display_server.window_get_mode() == WindowMode::MAXIMIZED;
    Self { pos, size, maxed }
  }

  pub fn from_variant(value: Option<Variant>) -> Self {
    if let Some(value) = value.and_then(|v| ok!(v.try_to::<Dictionary>())) {
      let pos = value.get(WinInfo::POS_KEY).and_then(geom::Pos::from_variant);
      let size = value.get(WinInfo::SIZE_KEY).and_then(geom::Size::from_variant);
      let maxed = value
        .get(WinInfo::MAXED_KEY)
        .and_then(|v| ok!(v.try_to::<bool>()))
        .unwrap_or(false);
      return Self { pos, size, maxed };
    }
    WinInfo::default()
  }

  pub fn to_variant(&self) -> Variant {
    let mut dict = Dictionary::new();
    dict.set(WinInfo::MAXED_KEY, Variant::from(self.maxed));

    if let Some(pos) = &self.pos {
      dict.set(WinInfo::POS_KEY, pos.to_variant());
    }

    if let Some(size) = &self.size {
      dict.set(WinInfo::SIZE_KEY, size.to_variant());
    }

    Variant::from(dict)
  }

  const POS_KEY: &'static str = "pos";
  const SIZE_KEY: &'static str = "size";
  const MAXED_KEY: &'static str = "maxed";
}

pub trait ToI32 {
  fn to_i32(self) -> Option<i32>;
}

impl ToI32 for f64 {
  fn to_i32(self) -> Option<i32> {
    let cast = self as i32;
    if cast as Self == self {
      return Some(cast);
    }
    None
  }
}

impl ToI32 for Variant {
  fn to_i32(self) -> Option<i32> {
    // JSON values are read as f64.
    ok!(self.try_to::<f64>())?.to_i32()
  }
}

pub trait ToU32 {
  fn to_u32(self) -> Option<u32>;
}

impl ToU32 for f64 {
  fn to_u32(self) -> Option<u32> {
    let cast = self as u32;
    if cast as Self == self {
      return Some(cast);
    }
    None
  }
}

impl ToU32 for i64 {
  fn to_u32(self) -> Option<u32> {
    let cast = self as u32;
    if cast as Self == self {
      return Some(cast);
    }
    None
  }
}

impl ToU32 for Variant {
  fn to_u32(self) -> Option<u32> {
    // JSON values are read as f64.
    ok!(self.try_to::<f64>())?.to_u32()
  }
}

/// Return the file stem portion of a path as a `&str`.
pub fn stem_str(path: &path::Path) -> Option<&str> {
  path.file_stem()?.to_str()
}

/// Return the folder of a path as a `GString`.
pub fn folder_gstring<P: AsRef<path::Path>>(path: P) -> Option<GString> {
  Some(folder_str(path.as_ref())?.into())
}

/// Return the folder of a path as a `&str`.
pub fn folder_str(path: &path::Path) -> Option<&str> {
  path.parent()?.to_str()
}

/// Get the OS specific downloads folder.
pub fn get_downloads_folder() -> GString {
  Os::singleton().get_system_dir(SystemDir::DOWNLOADS)
}

/// Load a text file.
pub fn load_text(path: &GString) -> Option<GString> {
  let file = FileAccess::open(path, ModeFlags::READ)?;
  Some(file.get_as_text())
}

/// Store a text file.
pub fn store_text(path: &GString, text: &GString) {
  let Some(mut file) = FileAccess::open(path, ModeFlags::WRITE) else {
    return;
  };

  file.store_string(text);
}

/// Request app permissions on Android.
pub fn request_permissions() {
  Os::singleton().request_permissions();
}

/// Make sure that a dialog window doesn't fall outside the edges of the main window.
pub fn adjust_dialog(dialog: &mut Gd<Window>) {
  if !dialog.is_visible() {
    return;
  }

  let Some(parent) = dialog.get_parent() else {
    return;
  };

  let Some(parent) = ok!(parent.try_cast::<Control>()) else {
    return;
  };

  const DECO: Vector2i = Vector2i::new(BORDER_WIDTH * 2, TITLE_HEIGHT + BORDER_HEIGHT);
  let max_size = parent.get_size();
  let max_size = Vector2i::new(max_size.x as i32, max_size.y as i32);
  let size = dialog.get_size() + DECO;

  // Make sure it's not bigger than the main window area.
  let new_size = Vector2i::new(size.x.min(max_size.x), size.y.min(max_size.y));
  if new_size != size {
    dialog.set_size(new_size - DECO);
  }

  const DELTA: Vector2i = Vector2i::new(BORDER_WIDTH, TITLE_HEIGHT);
  let pos = dialog.get_position();
  let mut new_pos = pos - DELTA;

  // Make sure it's not outside the main window area.
  if new_pos.x + size.x > max_size.x {
    new_pos.x = max_size.x - size.x;
  }

  if new_pos.y + size.y > max_size.y {
    new_pos.y = max_size.y - size.y;
  }

  if new_pos.x < 0 {
    new_pos.x = 0;
  }

  if new_pos.y < 0 {
    new_pos.y = 0;
  }

  new_pos += DELTA;
  if new_pos != pos {
    dialog.set_position(new_pos);
  }
}

/// RAII type that will reset feature reading when dropped.
pub struct Layer<'a>(vector::Layer<'a>);

impl<'a> Layer<'a> {
  pub fn new(layer: vector::Layer<'a>) -> Self {
    Self(layer)
  }
}

impl<'a> ops::Deref for Layer<'a> {
  type Target = vector::Layer<'a>;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl ops::DerefMut for Layer<'_> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}

impl Drop for Layer<'_> {
  fn drop(&mut self) {
    use vector::LayerAccess;

    self.0.reset_feature_reading();
  }
}
