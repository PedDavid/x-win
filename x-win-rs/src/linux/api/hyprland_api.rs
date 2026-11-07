//! Hyprland's request socket, not the generic Wayland foreign-toplevel protocol.
use std::{
  collections::{HashMap, HashSet},
  io::{self, Read, Write},
  os::unix::{
    ffi::OsStrExt,
    io::{AsRawFd, FromRawFd},
    net::UnixStream,
  },
  path::{Component, Path, PathBuf},
  sync::Mutex,
  time::{Duration, Instant},
};

use once_cell::sync::Lazy;
use serde_json::Value;

use super::common_api::{
  get_browser_url, get_window_memory_usage, get_window_path_name, init_entity,
};
use crate::common::{
  api::{empty_icon, Api},
  result::Result,
  x_win_struct::{icon_info::IconInfo, window_info::WindowInfo},
};

const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_millis(150);
static STATE: Lazy<Mutex<State>> = Lazy::new(|| Mutex::new(State::default()));

#[cfg(test)]
mod tests;

pub struct HyprlandApi;

pub(super) fn socket_path(runtime: &Path, signature: &str) -> Result<PathBuf> {
  if !runtime.is_absolute()
    || signature.is_empty()
    || signature.contains('\0')
    || Path::new(signature).components().count() != 1
    || !matches!(
      Path::new(signature).components().next(),
      Some(Component::Normal(_))
    )
  {
    return Err("Invalid Hyprland runtime directory or instance signature".into());
  }
  Ok(runtime.join("hypr").join(signature).join(".socket.sock"))
}

fn discovery() -> Result<PathBuf> {
  socket_path(
    Path::new(&std::env::var_os("XDG_RUNTIME_DIR").ok_or("Hyprland requires XDG_RUNTIME_DIR")?),
    &std::env::var("HYPRLAND_INSTANCE_SIGNATURE")?,
  )
}

fn wait(stream: &UnixStream, events: i16, deadline: Instant) -> io::Result<()> {
  loop {
    let remaining = deadline
      .checked_duration_since(Instant::now())
      .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Hyprland IPC deadline exceeded"))?;
    let mut fd = libc::pollfd {
      fd: stream.as_raw_fd(),
      events,
      revents: 0,
    };
    // SAFETY: fd points to one initialized pollfd for the duration of poll.
    let result = unsafe { libc::poll(&mut fd, 1, remaining.as_millis().max(1) as i32) };
    if result > 0 {
      return Ok(());
    }
    if result == 0 {
      return Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Hyprland IPC timeout",
      ));
    }
    let error = io::Error::last_os_error();
    if error.kind() != io::ErrorKind::Interrupted {
      return Err(error);
    }
  }
}

fn connect(path: &Path, deadline: Instant) -> io::Result<UnixStream> {
  // Nonblocking connect also bounds a full listen backlog (SO_SNDTIMEO alone does not).
  // SAFETY: socket has no pointer arguments; ownership is immediately transferred below.
  let fd = unsafe {
    libc::socket(
      libc::AF_UNIX,
      libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
      0,
    )
  };
  if fd < 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: fd is a newly created, exclusively owned stream socket.
  let stream = unsafe { UnixStream::from_raw_fd(fd) };
  // SAFETY: all-zero is valid for sockaddr_un, populated before use.
  let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
  address.sun_family = libc::AF_UNIX as libc::sa_family_t;
  let bytes = path.as_os_str().as_bytes();
  if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "Hyprland socket path too long or invalid",
    ));
  }
  for (dest, byte) in address.sun_path.iter_mut().zip(bytes) {
    *dest = *byte as _;
  }
  // SAFETY: address is an initialized sockaddr_un and the supplied length matches it.
  let result = unsafe {
    libc::connect(
      fd,
      &address as *const _ as *const libc::sockaddr,
      std::mem::size_of_val(&address) as libc::socklen_t,
    )
  };
  if result < 0 {
    let error = io::Error::last_os_error();
    // Linux AF_UNIX EAGAIN means backlog full, not an in-progress connection.
    if error.raw_os_error() != Some(libc::EINPROGRESS) {
      return Err(error);
    }
    wait(&stream, libc::POLLOUT, deadline)?;
    if let Some(error) = stream.take_error()? {
      return Err(error);
    }
  }
  Ok(stream)
}

fn exchange(mut stream: UnixStream, command: &[u8], deadline: Instant) -> io::Result<Vec<u8>> {
  let mut sent = 0;
  while sent < command.len() {
    wait(&stream, libc::POLLOUT, deadline)?;
    match stream.write(&command[sent..]) {
      Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
      Ok(n) => sent += n,
      Err(e)
        if matches!(
          e.kind(),
          io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) =>
      {
        continue
      }
      Err(e) => return Err(e),
    }
  }
  // The server closes the connection after its response. No newline/length prefix.
  let mut response = Vec::new();
  let mut buffer = [0; 8192];
  loop {
    wait(&stream, libc::POLLIN, deadline)?;
    match stream.read(&mut buffer) {
      Ok(0) => return Ok(response),
      Ok(n) => {
        if response.len() + n > MAX_RESPONSE {
          return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Hyprland response too large",
          ));
        }
        response.extend_from_slice(&buffer[..n]);
      }
      Err(e)
        if matches!(
          e.kind(),
          io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) =>
      {
        continue
      }
      Err(e) => return Err(e),
    }
  }
}

fn query(path: &Path, command: &[u8]) -> Result<Value> {
  let deadline = Instant::now() + TIMEOUT;
  let stream = connect(path, deadline)?;
  Ok(serde_json::from_slice(&exchange(
    stream, command, deadline,
  )?)?)
}

#[derive(Default)]
struct State {
  session: Option<PathBuf>,
  next_id: u32,
  windows: HashMap<String, (u32, String)>,
}

fn address(value: &Value) -> Result<String> {
  let raw = value
    .get("address")
    .and_then(Value::as_str)
    .ok_or("Missing Hyprland window address")?;
  let number = raw
    .strip_prefix("0x")
    .and_then(|s| u64::from_str_radix(s, 16).ok())
    .filter(|n| *n != 0)
    .ok_or("Invalid Hyprland window address")?;
  Ok(format!("{number:x}"))
}

impl State {
  fn reset(&mut self) {
    self.session = None;
    self.windows.clear(); // Never reuse numeric IDs, even after a disconnect.
  }

  fn reconcile(&mut self, clients: &Value) -> Result<()> {
    let clients = clients.as_array().ok_or("Expected Hyprland client array")?;
    let mut seen = HashSet::new();
    for client in clients {
      let key = address(client)?;
      if !seen.insert(key.clone()) {
        return Err("Duplicate Hyprland window address".into());
      }
      // stableId is optional; PID is only a reuse hint, never the window identity.
      let generation = format!(
        "{}:{}",
        client.get("pid").unwrap_or(&Value::Null),
        client.get("stableId").unwrap_or(&Value::Null)
      );
      if self
        .windows
        .get(&key)
        .map(|(_, old)| old != &generation)
        .unwrap_or(true)
      {
        self.next_id = self
          .next_id
          .checked_add(1)
          .ok_or("Hyprland window ID space exhausted")?;
        self.windows.insert(key, (self.next_id, generation));
      }
    }
    self.windows.retain(|key, _| seen.contains(key));
    Ok(())
  }

  fn snapshot(&mut self, active: bool) -> Result<Vec<WindowInfo>> {
    match discovery() {
      Ok(path) => self.snapshot_at(&path, active),
      Err(error) => {
        self.reset();
        Err(error)
      }
    }
  }

  fn snapshot_at(&mut self, path: &Path, active: bool) -> Result<Vec<WindowInfo>> {
    let result = (|| {
      let path = path.to_owned();
      if self.session.as_ref() != Some(&path) {
        self.reset();
        self.session = Some(path.clone());
      }
      let clients = query(&path, b"j/clients")?;
      self.reconcile(&clients)?;
      if active {
        let value = query(&path, b"j/activewindow")?;
        if value.as_object().map(|v| v.is_empty()).unwrap_or(false) {
          return Ok(vec![init_entity()]);
        }
        let key = address(&value)?;
        // Close/open race between the two requests: retry on the next poll, not stale data.
        match self.windows.get(&key) {
          Some((id, _)) => Ok(vec![window_info(&value, *id, &Proc)?]),
          None => Ok(vec![init_entity()]),
        }
      } else {
        clients
          .as_array()
          .ok_or("Expected client array")?
          .iter()
          .map(|value| {
            let key = address(value)?;
            let id = self.windows.get(&key).ok_or("Missing window identity")?.0;
            window_info(value, id, &Proc)
          })
          .collect()
      }
    })();
    if result.is_err() {
      self.reset();
    }
    result
  }
}

trait ProcessMetadata {
  fn executable(&self, pid: u32) -> Result<(String, String)>;
  fn memory(&self, pid: u32) -> Result<u32>;
}
struct Proc;
impl ProcessMetadata for Proc {
  fn executable(&self, pid: u32) -> Result<(String, String)> {
    get_window_path_name(pid)
  }
  fn memory(&self, pid: u32) -> Result<u32> {
    get_window_memory_usage(pid)
  }
}

fn window_info(value: &Value, id: u32, proc: &impl ProcessMetadata) -> Result<WindowInfo> {
  address(value)?;
  let mut info = init_entity();
  info.id = id;
  info.title = value
    .get("title")
    .and_then(Value::as_str)
    .unwrap_or_default()
    .into();
  info.info.name = value
    .get("class")
    .and_then(Value::as_str)
    .unwrap_or_default()
    .into();
  info.info.process_id = value
    .get("pid")
    .and_then(Value::as_u64)
    .and_then(|pid| u32::try_from(pid).ok())
    .unwrap_or(0);
  if info.info.process_id != 0 {
    if let Ok((path, name)) = proc.executable(info.info.process_id) {
      info.info.path = path;
      info.info.exec_name = name;
    }
    info.usage.memory = proc.memory(info.info.process_id).unwrap_or(0);
  }
  let component = |key: &str, index: usize| {
    value
      .get(key)
      .and_then(|v| v.get(index))
      .and_then(Value::as_i64)
      .and_then(|v| i32::try_from(v).ok())
      .unwrap_or(0)
  };
  info.position.x = component("at", 0);
  info.position.y = component("at", 1);
  info.position.width = component("size", 0).max(0);
  info.position.height = component("size", 1).max(0);
  // Modern Hyprland: 1=maximized, 2=fullscreen. Legacy schemas used a boolean.
  info.position.is_full_screen = match value.get("fullscreen") {
    Some(Value::Bool(fullscreen)) => *fullscreen,
    Some(Value::Number(mode)) => mode.as_u64().map(|mode| mode & 2 != 0).unwrap_or(false),
    _ => false,
  };
  Ok(info)
}

impl Api for HyprlandApi {
  fn get_active_window(&self) -> Result<WindowInfo> {
    let mut state = STATE
      .try_lock()
      .map_err(|_| "Hyprland query already in progress")?;
    state
      .snapshot(true)?
      .into_iter()
      .next()
      .ok_or_else(|| "Missing active window result".into())
  }
  fn get_open_windows(&self) -> Result<Vec<WindowInfo>> {
    STATE
      .try_lock()
      .map_err(|_| "Hyprland query already in progress")?
      .snapshot(false)
  }
  fn get_app_icon(&self, _: &WindowInfo) -> Result<IconInfo> {
    Ok(empty_icon())
  }
  fn get_browser_url(&self, _: &WindowInfo) -> Result<String> {
    Ok(get_browser_url())
  }
}
