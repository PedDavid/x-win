mod common_api;
mod common_zbus;
mod gnome_shell;
mod hyprland_api;
mod wayland_api;
mod wayland_eval_api;
mod wayland_extension_api;
mod x11_api;

use common_api::is_wayland_desktop;
use hyprland_api::HyprlandApi;
use wayland_api::WaylandApi;
use x11_api::X11Api;

use crate::common::{
  api::Api,
  result::Result,
  x_win_struct::{icon_info::IconInfo, window_info::WindowInfo},
};

pub trait APIGnome {
  fn install_extension() -> Result<bool>;
  fn uninstall_extension() -> Result<bool>;
  fn enable_extension() -> Result<bool>;
  fn disable_extension() -> Result<bool>;
  fn is_installed_extension() -> Result<bool>;
  fn is_enabled_extension() -> Result<bool>;
}

#[cfg(test)]
mod routing_tests {
  use super::*;
  #[test]
  fn compositor_selection() {
    assert_eq!(select_backend(true, "Hyprland", ""), Backend::Hyprland);
    assert_eq!(select_backend(true, "", "synthetic"), Backend::Hyprland);
    assert_eq!(select_backend(true, "ubuntu:GNOME", ""), Backend::Gnome);
    assert_eq!(select_backend(false, "GNOME", ""), Backend::X11);
    assert_eq!(select_backend(false, "", ""), Backend::X11);
    assert_eq!(select_backend(true, "sway", ""), Backend::Unsupported);
    assert_eq!(select_backend(true, "", ""), Backend::Unsupported);
  }
}

pub struct LinuxAPI {}

#[derive(Debug, PartialEq)]
enum Backend {
  Hyprland,
  Gnome,
  X11,
  Unsupported,
}

fn select_backend(wayland: bool, desktop: &str, signature: &str) -> Backend {
  let desktops: Vec<_> = desktop.split(':').collect();
  if desktops.iter().any(|d| d.eq_ignore_ascii_case("Hyprland")) || !signature.is_empty() {
    Backend::Hyprland
  } else if wayland {
    if desktops.iter().any(|d| d.eq_ignore_ascii_case("GNOME")) {
      Backend::Gnome
    } else {
      Backend::Unsupported
    }
  } else {
    Backend::X11
  }
}

fn backend() -> Backend {
  select_backend(
    is_wayland_desktop(),
    &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
    &std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default(),
  )
}

fn platform() -> Result<Box<dyn Api>> {
  match backend() {
    Backend::Hyprland => Ok(Box::new(HyprlandApi)),
    Backend::Gnome => Ok(Box::new(WaylandApi {})),
    Backend::X11 => Ok(Box::new(X11Api {})),
    Backend::Unsupported => {
      Err("Unsupported Wayland compositor (supported: Hyprland, GNOME)".into())
    }
  }
}

/**
 * Impl. for windows system
 */
impl Api for LinuxAPI {
  fn get_active_window(&self) -> crate::common::result::Result<WindowInfo> {
    platform()?.get_active_window()
  }

  fn get_open_windows(&self) -> crate::common::result::Result<Vec<WindowInfo>> {
    platform()?.get_open_windows()
  }

  fn get_app_icon(&self, window_info: &WindowInfo) -> crate::common::result::Result<IconInfo> {
    platform()?.get_app_icon(window_info)
  }

  fn get_browser_url(&self, window_info: &WindowInfo) -> crate::common::result::Result<String> {
    platform()?.get_browser_url(window_info)
  }
}

impl APIGnome for LinuxAPI {
  fn install_extension() -> Result<bool> {
    if backend() == Backend::Gnome {
      WaylandApi::install_extension()
    } else {
      Ok(false)
    }
  }

  fn uninstall_extension() -> Result<bool> {
    if backend() == Backend::Gnome {
      WaylandApi::uninstall_extension()
    } else {
      Ok(false)
    }
  }

  fn enable_extension() -> Result<bool> {
    if backend() == Backend::Gnome {
      WaylandApi::enable_extension()
    } else {
      Ok(false)
    }
  }

  fn disable_extension() -> Result<bool> {
    if backend() == Backend::Gnome {
      WaylandApi::disable_extension()
    } else {
      Ok(false)
    }
  }

  fn is_installed_extension() -> Result<bool> {
    if backend() == Backend::Gnome {
      WaylandApi::is_installed_extension()
    } else {
      Ok(false)
    }
  }

  fn is_enabled_extension() -> Result<bool> {
    if backend() == Backend::Gnome {
      WaylandApi::is_enabled_extension()
    } else {
      Ok(false)
    }
  }
}
