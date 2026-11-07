//! Shared polling policy: errors invalidate activity, but never end monitoring.
use std::{
  sync::mpsc::{Receiver, TryRecvError},
  time::Duration,
};
use x_win::{empty_entity, Result, WindowInfo};

#[derive(Default)]
struct Changes {
  previous: Option<(u32, String)>,
  failed: bool,
}

impl Changes {
  fn update(&mut self, window: &WindowInfo) -> bool {
    let changed = self
      .previous
      .as_ref()
      .map(|(id, title)| *id != window.id || title != &window.title)
      .unwrap_or(true);
    self.previous = Some((window.id, window.title.clone()));
    self.failed = false;
    changed
  }

  fn fail(&mut self) -> bool {
    let report = !self.failed;
    self.previous = None;
    self.failed = true;
    report
  }
}

pub fn run(
  receiver: Receiver<()>,
  interval: Duration,
  mut poll: impl FnMut() -> Result<WindowInfo>,
  mut notify: impl FnMut(Result<WindowInfo>),
) {
  let mut changes = Changes::default();
  loop {
    match receiver.try_recv() {
      Ok(_) | Err(TryRecvError::Disconnected) => break,
      Err(TryRecvError::Empty) => {}
    }
    let delay = match poll() {
      Ok(window) => {
        if changes.update(&window) {
          notify(Ok(window));
        }
        interval
      }
      Err(error) => {
        if changes.fail() {
          // An explicit no-focus transition prevents stale activity through an outage.
          notify(Ok(empty_entity()));
          notify(Err(error));
        }
        interval.max(Duration::from_millis(250))
      }
    };
    // Unlike sleep, this is immediately interruptible even with a very long interval.
    if !matches!(
      receiver.recv_timeout(delay),
      Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ) {
      break;
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::mpsc;

  #[test]
  fn focus_title_empty_and_recovery() {
    let mut changes = Changes::default();
    let mut window = empty_entity();
    assert!(changes.update(&window));
    assert!(!changes.update(&window));
    window.id = 1;
    assert!(changes.update(&window));
    window.title = "Synthetic title".into();
    assert!(changes.update(&window));
    assert!(!changes.update(&window));
    assert!(changes.update(&empty_entity()));
    assert!(changes.update(&window));
    assert!(changes.fail());
    assert!(!changes.fail());
    assert!(changes.update(&window));
  }

  #[test]
  fn retries_and_interruptible_cancellation() {
    for _ in 0..10 {
      let (stop, receiver) = mpsc::channel();
      let (events, seen) = mpsc::channel();
      let handle = std::thread::spawn(move || {
        let mut attempts = 0;
        run(
          receiver,
          Duration::from_millis(1),
          || {
            attempts += 1;
            if attempts == 1 {
              Err("Synthetic disconnect".into())
            } else {
              let mut w = empty_entity();
              w.id = 7;
              Ok(w)
            }
          },
          |event| {
            events.send(event.map(|w| w.id).map_err(|_| ())).unwrap();
          },
        );
      });
      assert_eq!(seen.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(0));
      assert_eq!(seen.recv_timeout(Duration::from_secs(1)).unwrap(), Err(()));
      assert_eq!(seen.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(7));
      stop.send(()).unwrap();
      handle.join().unwrap();
    }
    let (stop, receiver) = mpsc::channel();
    let (events, seen) = mpsc::channel();
    let handle = std::thread::spawn(move || {
      run(
        receiver,
        Duration::from_secs(3600),
        || Ok(empty_entity()),
        |_| {
          events.send(()).unwrap();
        },
      )
    });
    seen.recv_timeout(Duration::from_secs(1)).unwrap();
    let start = std::time::Instant::now();
    stop.send(()).unwrap();
    handle.join().unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
  }
}
