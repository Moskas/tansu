//! Metadata formatting and small system queries (owner names, free space, hostname).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// `drwxr-xr-x`-style permission string, including the file type character.
pub fn format_permissions(mode: u32) -> String {
  let kind = match mode & (libc::S_IFMT) {
    m if m == libc::S_IFDIR => 'd',
    m if m == libc::S_IFLNK => 'l',
    m if m == libc::S_IFIFO => 'p',
    m if m == libc::S_IFSOCK => 's',
    m if m == libc::S_IFCHR => 'c',
    m if m == libc::S_IFBLK => 'b',
    _ => '-',
  };
  let mut s = String::with_capacity(10);
  s.push(kind);
  let special = [(0o4000, 's', 'S'), (0o2000, 's', 'S'), (0o1000, 't', 'T')];
  for (i, (special_bit, set_exec, set_noexec)) in special.into_iter().enumerate() {
    let shift = 6 - 3 * i as u32;
    let bits = (mode >> shift) & 0o7;
    s.push(if bits & 4 != 0 { 'r' } else { '-' });
    s.push(if bits & 2 != 0 { 'w' } else { '-' });
    let exec = bits & 1 != 0;
    s.push(match (mode & special_bit != 0, exec) {
      (true, true) => set_exec,
      (true, false) => set_noexec,
      (false, true) => 'x',
      (false, false) => '-',
    });
  }
  s
}

/// Splits a byte count into a 3-significant-digit number and a unit (`B`, `K`, `M`, ...).
pub fn human_size_parts(bytes: u64) -> (String, &'static str) {
  const UNITS: [&str; 7] = ["B", "K", "M", "G", "T", "P", "E"];
  if bytes < 1024 {
    return (bytes.to_string(), UNITS[0]);
  }
  let mut value = bytes as f64;
  let mut unit = 0;
  while value >= 1000.0 && unit < UNITS.len() - 1 {
    value /= 1024.0;
    unit += 1;
  }
  let text = if value < 9.995 {
    format!("{value:.2}")
  } else if value < 99.95 {
    format!("{value:.1}")
  } else {
    format!("{value:.0}")
  };
  (text, UNITS[unit])
}

/// `127 M` style size used in the file columns.
pub fn human_size(bytes: u64) -> String {
  let (n, u) = human_size_parts(bytes);
  format!("{n} {u}")
}

/// `5.58G` style size used in the status bar.
pub fn human_size_compact(bytes: u64) -> String {
  let (n, u) = human_size_parts(bytes);
  if u == "B" { n } else { format!("{n}{u}") }
}

/// Formats a Unix timestamp as local `YYYY-MM-DD HH:MM`.
pub fn format_mtime(secs: i64) -> String {
  let t = secs as libc::time_t;
  // SAFETY: `tm` is plain data; localtime_r only writes into it and is thread-safe.
  let tm = unsafe {
    let mut tm: libc::tm = std::mem::zeroed();
    if libc::localtime_r(&t, &mut tm).is_null() {
      return "????-??-?? ??:??".to_string();
    }
    tm
  };
  format!(
    "{:04}-{:02}-{:02} {:02}:{:02}",
    tm.tm_year + 1900,
    tm.tm_mon + 1,
    tm.tm_mday,
    tm.tm_hour,
    tm.tm_min
  )
}

/// Loads the local timezone once so later `format_mtime` calls do not touch the filesystem.
pub fn init_timezone() {
  // SAFETY: tzset has no preconditions; called once at startup before other threads format time.
  unsafe { tzset() };
}

unsafe extern "C" {
  fn tzset();
}

/// Free bytes available to unprivileged users on the filesystem containing `path`.
pub fn free_space(path: &Path) -> Option<u64> {
  let c = CString::new(path.as_os_str().as_bytes()).ok()?;
  // SAFETY: `st` is plain data and `c` is a valid NUL-terminated string.
  let st = unsafe {
    let mut st: libc::statvfs = std::mem::zeroed();
    if libc::statvfs(c.as_ptr(), &mut st) != 0 {
      return None;
    }
    st
  };
  #[allow(clippy::unnecessary_cast)]
  Some(st.f_bavail as u64 * st.f_frsize as u64)
}

pub fn user_name(uid: u32) -> Option<String> {
  let mut buf = vec![0 as libc::c_char; 4096];
  // SAFETY: getpwuid_r writes into `pwd` and `buf` (with its length) and sets `result`.
  unsafe {
    let mut pwd: libc::passwd = std::mem::zeroed();
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result);
    if rc != 0 || result.is_null() || pwd.pw_name.is_null() {
      return None;
    }
    Some(CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned())
  }
}

pub fn group_name(gid: u32) -> Option<String> {
  let mut buf = vec![0 as libc::c_char; 4096];
  // SAFETY: getgrgid_r writes into `grp` and `buf` (with its length) and sets `result`.
  unsafe {
    let mut grp: libc::group = std::mem::zeroed();
    let mut result: *mut libc::group = std::ptr::null_mut();
    let rc = libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut result);
    if rc != 0 || result.is_null() || grp.gr_name.is_null() {
      return None;
    }
    Some(CStr::from_ptr(grp.gr_name).to_string_lossy().into_owned())
  }
}

/// Memoizes uid/gid to name lookups for the duration of one listing.
#[derive(Default)]
pub struct NameCache {
  users: HashMap<u32, String>,
  groups: HashMap<u32, String>,
}

impl NameCache {
  pub fn resolve(&mut self, uid: u32, gid: u32) -> (String, String) {
    let user = self
      .users
      .entry(uid)
      .or_insert_with(|| user_name(uid).unwrap_or_else(|| uid.to_string()))
      .clone();
    let group = self
      .groups
      .entry(gid)
      .or_insert_with(|| group_name(gid).unwrap_or_else(|| gid.to_string()))
      .clone();
    (user, group)
  }
}

pub fn hostname() -> String {
  let mut buf = [0 as libc::c_char; 256];
  // SAFETY: the buffer length is passed; the result is NUL-terminated on success.
  let ok = unsafe { libc::gethostname(buf.as_mut_ptr(), buf.len() - 1) } == 0;
  if !ok {
    return "localhost".to_string();
  }
  // SAFETY: the last byte is never written, so the buffer is NUL-terminated.
  unsafe { CStr::from_ptr(buf.as_ptr()) }
    .to_string_lossy()
    .into_owned()
}

pub fn current_user() -> String {
  // SAFETY: getuid cannot fail.
  let uid = unsafe { libc::getuid() };
  user_name(uid)
    .or_else(|| std::env::var("USER").ok())
    .unwrap_or_else(|| uid.to_string())
}

/// `user@host` for the header line.
pub fn user_at_host() -> String {
  format!("{}@{}", current_user(), hostname())
}
