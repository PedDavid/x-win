//! Live integration; deliberately does not print titles, executable paths or classes.
use std::time::Instant;
use x_win::{get_active_window, get_open_windows, get_window_icon, Result};

fn main() -> Result<()> {
  let active = get_active_window()?;
  let windows = get_open_windows()?;
  assert_eq!(active.os, "linux");
  let ids: std::collections::HashSet<_> = windows.iter().map(|w| w.id).collect();
  assert_eq!(ids.len(), windows.len());
  assert!(windows.iter().all(|w| w.id != 0));
  assert!(get_window_icon(&active)?.data.is_empty());
  let start = Instant::now();
  for _ in 0..100 {
    get_active_window()?;
  }
  println!(
    "focused={}, windows={}, mean_query_us={}",
    active.id != 0,
    windows.len(),
    start.elapsed().as_micros() / 100
  );
  Ok(())
}
