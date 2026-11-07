use super::*;
use serde_json::json;
use std::{
  os::unix::net::UnixListener,
  sync::atomic::{AtomicUsize, Ordering},
  thread,
};

fn client(address: &str) -> Value {
  json!({"address":address,"pid":123,"title":"Synthetic editor","class":"fixture.Editor","at":[-10,20],"size":[800,600],"fullscreen":2,"xwayland":false,"unknown":"ignored"})
}

struct FixtureProc;
impl ProcessMetadata for FixtureProc {
  fn executable(&self, _: u32) -> Result<(String, String)> {
    Ok(("/fixture/editor".into(), "editor".into()))
  }
  fn memory(&self, _: u32) -> Result<u32> {
    Ok(1234)
  }
}

struct MissingProc;
impl ProcessMetadata for MissingProc {
  fn executable(&self, _: u32) -> Result<(String, String)> {
    Err(io::Error::from(io::ErrorKind::PermissionDenied).into())
  }
  fn memory(&self, _: u32) -> Result<u32> {
    Err(io::Error::from(io::ErrorKind::NotFound).into())
  }
}

#[test]
fn metadata_and_missing_process() {
  let value = client("0x1234");
  let available = window_info(&value, 1, &FixtureProc).unwrap();
  assert_eq!(available.info.path, "/fixture/editor");
  assert_eq!(available.info.exec_name, "editor");
  assert_eq!(available.usage.memory, 1234); // Existing units passed through without conversion.
  let info = window_info(&value, 1, &MissingProc).unwrap();
  assert_eq!(info.id, 1);
  assert_eq!(info.os, "linux");
  assert_eq!(info.title, "Synthetic editor");
  assert_eq!(info.info.name, "fixture.Editor");
  assert_eq!(info.info.process_id, 123);
  assert_eq!(info.info.path, "");
  assert_eq!(info.usage.memory, 0);
  assert_eq!(
    (
      info.position.x,
      info.position.y,
      info.position.width,
      info.position.height
    ),
    (-10, 20, 800, 600)
  );
  assert!(info.position.is_full_screen);
  let minimal = json!({"address":"0x55"});
  let info = window_info(&minimal, 2, &MissingProc).unwrap();
  assert_eq!(info.info.process_id, 0);
  assert_eq!(info.title, "");
  assert!(window_info(&json!({}), 1, &MissingProc).is_err());
  assert!(window_info(&json!({"address":"garbage"}), 1, &MissingProc).is_err());
}

#[test]
fn fullscreen_variants_and_xwayland() {
  for (mode, expected) in [
    (json!(true), true),
    (json!(false), false),
    (json!(0), false),
    (json!(1), false),
    (json!(2), true),
    (json!(3), true),
    (Value::Null, false),
  ] {
    let mut value = client("0x1234");
    value["fullscreen"] = mode;
    value["xwayland"] = json!(true);
    assert_eq!(
      window_info(&value, 1, &MissingProc)
        .unwrap()
        .position
        .is_full_screen,
      expected
    );
  }
}

#[test]
fn identities_cleanup_reuse_and_reconnect() {
  let a = client("0x100000001");
  let b = client("0x200000001"); // Same low 32 bits and PID: still distinct.
  let mut state = State::default();
  state.reconcile(&json!([a, b])).unwrap();
  let id = state.windows["100000001"].0;
  assert_ne!(id, state.windows["200000001"].0);
  let mut changed = a.clone();
  changed["title"] = json!("Synthetic renamed title");
  state.reconcile(&json!([changed, b])).unwrap();
  assert_eq!(id, state.windows["100000001"].0);
  state.reconcile(&json!([b])).unwrap();
  assert_eq!(state.windows.len(), 1);
  state.reconcile(&json!([a, b])).unwrap();
  assert_ne!(id, state.windows["100000001"].0);
  let before = state.windows["100000001"].0;
  state.reset();
  state.reconcile(&json!([a])).unwrap();
  assert_ne!(before, state.windows["100000001"].0);
  let before = state.windows["100000001"].0;
  let mut reused = a.clone();
  reused["stableId"] = json!("different");
  state.reconcile(&json!([reused])).unwrap();
  assert_ne!(before, state.windows["100000001"].0);
  assert!(state.reconcile(&json!({})).is_err());
  assert!(state.reconcile(&json!([a, a])).is_err());
  state.next_id = u32::MAX;
  assert!(state.reconcile(&json!([b])).is_err());
}

#[test]
fn safe_discovery() {
  assert_eq!(
    socket_path(Path::new("/run/user/1000"), "synthetic_123").unwrap(),
    Path::new("/run/user/1000/hypr/synthetic_123/.socket.sock")
  );
  for signature in ["", "..", ".", "/escape", "a/b", "a\0b"] {
    assert!(socket_path(Path::new("/run/user/1000"), signature).is_err());
  }
  assert!(socket_path(Path::new("relative"), "test").is_err());
}

fn pair(server: impl FnOnce(UnixStream) + Send + 'static) -> (UnixStream, thread::JoinHandle<()>) {
  let (client, peer) = UnixStream::pair().unwrap();
  client.set_nonblocking(true).unwrap();
  (client, thread::spawn(move || server(peer)))
}

#[test]
fn transport_success_truncation_and_malformed() {
  for bytes in [
    b"{}".to_vec(),
    b"{\"title\":".to_vec(),
    b"not json".to_vec(),
  ] {
    let expected = bytes.clone();
    let (client, handle) = pair(move |mut peer| {
      let mut request = [0; 14];
      peer.read_exact(&mut request).unwrap();
      assert_eq!(&request, b"j/activewindow");
      peer.write_all(&bytes).unwrap();
    });
    let response = exchange(client, b"j/activewindow", Instant::now() + TIMEOUT).unwrap();
    assert_eq!(response, expected);
    assert_eq!(
      serde_json::from_slice::<Value>(&response).is_ok(),
      response == b"{}"
    );
    handle.join().unwrap();
  }
}

#[test]
fn transport_timeout_oversize_disconnect_and_trickle() {
  for mode in 0..4 {
    let (client, handle) = pair(move |mut peer| {
      let mut request = [0; 9];
      peer.read_exact(&mut request).unwrap();
      match mode {
        0 => thread::sleep(Duration::from_millis(250)),
        1 => {
          let _ = peer.write_all(&vec![b'x'; MAX_RESPONSE + 1]);
        }
        2 => {}
        _ => {
          for _ in 0..40 {
            if peer.write_all(b" ").is_err() {
              break;
            }
            thread::sleep(Duration::from_millis(10));
          }
        }
      }
    });
    let start = Instant::now();
    let result = exchange(client, b"j/clients", Instant::now() + TIMEOUT);
    if mode == 2 {
      assert!(result.unwrap().is_empty());
    } else {
      assert!(result.is_err());
    }
    assert!(start.elapsed() < Duration::from_millis(500));
    handle.join().unwrap();
  }
}

#[test]
fn real_socket_query_retry_and_no_focus() {
  static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
  let dir = std::env::temp_dir().join(format!(
    "x-win-test-{}-{}",
    std::process::id(),
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
  ));
  std::fs::create_dir(&dir).unwrap();
  let path = dir.join("socket");
  assert!(query(&path, b"j/activewindow").is_err());
  let listener = UnixListener::bind(&path).unwrap();
  let handle = thread::spawn(move || {
    for response in [b"invalid".as_slice(), b"{}".as_slice()] {
      let (mut peer, _) = listener.accept().unwrap();
      let mut request = [0; 14];
      peer.read_exact(&mut request).unwrap();
      assert_eq!(&request, b"j/activewindow");
      peer.write_all(response).unwrap();
    }
  });
  assert!(query(&path, b"j/activewindow").is_err());
  assert_eq!(query(&path, b"j/activewindow").unwrap(), json!({}));
  handle.join().unwrap();
  std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn snapshots_empty_close_race_disconnect_and_restoration() {
  let dir = std::env::temp_dir().join(format!("x-win-snapshot-{}", std::process::id()));
  std::fs::create_dir(&dir).unwrap();
  let path = dir.join("socket");
  let listener = UnixListener::bind(&path).unwrap();
  let a = client("0x123");
  let responses = [
    json!([a]).to_string(),
    a.to_string(),
    json!([a]).to_string(),
    "{}".into(),
    "[]".into(),
    a.to_string(), // close race
    "malformed".into(),
    json!([a]).to_string(),
    a.to_string(),
  ];
  let handle = thread::spawn(move || {
    for response in responses {
      let (mut peer, _) = listener.accept().unwrap();
      let mut request = [0; 64];
      assert!(peer.read(&mut request).unwrap() > 0);
      peer.write_all(response.as_bytes()).unwrap();
    }
  });
  let mut state = State::default();
  let first = state.snapshot_at(&path, true).unwrap().remove(0);
  assert_ne!(first.id, 0);
  assert_eq!(state.snapshot_at(&path, true).unwrap()[0].id, 0);
  assert_eq!(state.snapshot_at(&path, true).unwrap()[0].id, 0);
  assert!(state.windows.is_empty());
  assert!(state.snapshot_at(&path, true).is_err());
  assert!(state.session.is_none());
  let restored = state.snapshot_at(&path, true).unwrap().remove(0);
  assert_ne!(first.id, restored.id);
  handle.join().unwrap();
  std::fs::remove_dir_all(dir).unwrap();
}
