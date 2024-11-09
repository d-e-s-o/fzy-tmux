// Copyright (C) 2024 Daniel Mueller <deso@posteo.net>
// SPDX-License-Identifier: GPL-3.0-or-later

use std::env;
use std::ffi::OsStr;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::stdin;
use std::io::stdout;
use std::io::Read;
use std::iter::once;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::process::exit;
use std::process::Command;
use std::process::Stdio;
use std::str;
use std::thread;

use anyhow::ensure;
use anyhow::Context as _;
use anyhow::Result;

use libc::mkfifo;
use libc::mode_t;

use libc::S_IRWXU;
use tempfile::TempDir;


fn make_fifo(path: &Path, mode: mode_t) -> Result<()> {
  let cpath = path
    .as_os_str()
    .as_bytes()
    .iter()
    .copied()
    .chain(once(b'\0'))
    .collect::<Vec<_>>();

  let rc = unsafe { mkfifo(cpath.as_ptr().cast(), mode) };
  ensure!(rc == 0, "failed to create FIFO at `{}`", path.display());
  Ok(())
}


/// Filter the contents of the `TMUX` environment variable.
fn filter_tmux(tmux: &OsStr) -> &OsStr {
  let bytes = tmux.as_bytes();
  let mut comma_count = 0;
  let end = bytes
    .iter()
    .enumerate()
    .find(|&(_, &b)| {
      if b == b',' {
        comma_count += 1;
      }
      comma_count == 2
    })
    .map_or(bytes.len(), |(i, _)| i);
  OsStr::from_bytes(&bytes[..end])
}


fn main() -> Result<()> {
  let tmux = env::var_os("TMUX").context("TMUX variable not found")?;
  let tmux = filter_tmux(&tmux);

  // Create a bunch of named FIFOs that we can use for communicating
  // with the fzy instance running inside tmux.
  let tmp_dir = TempDir::new().context("failed to create temporary directory")?;
  let fifo_in = tmp_dir.path().join("in");
  let fifo_out = tmp_dir.path().join("out");
  let fifo_ret = tmp_dir.path().join("ret");

  for fifo in [&fifo_in, &fifo_out, &fifo_ret] {
    let () = make_fifo(fifo, S_IRWXU)
      .with_context(|| format!("failed to create FIFO `{}`", fifo.display()))?;
  }

  let fzy = format!(
    "fzy --lines 50 $* < '{}' > '{}' 2>&1 && echo 0 > '{ret}' || echo 1 > '{ret}'",
    fifo_in.display(),
    fifo_out.display(),
    ret = fifo_ret.display(),
  );

  let _child = Command::new("tmux")
    .env_clear()
    .env("TMUX", tmux)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .args(["set-window-option", "synchronize-panes", "off", ";"])
    .args(["set-window-option", "remain-on-exit", "off", ";"])
    .args(["split-window", &fzy])
    .spawn()
    .context("failed to execute `tmux` command")?;

  let mut fifo_in = OpenOptions::new()
    .write(true)
    .open(&fifo_in)
    .context("failed to open stdin FIFO")?;

  // Transparently forward our program's input to the fzy instance we
  // spawned.
  let _thread = thread::spawn(move || {
    if let Err(err) = io::copy(&mut stdin().lock(), &mut fifo_in) {
      eprintln!("failed to pipe standard input: {err}")
    }
  });

  let mut fifo_out = File::open(&fifo_out).context("failed to open stdout FIFO")?;
  let _cnt =
    io::copy(&mut fifo_out, &mut stdout().lock()).context("failed to copy standard output")?;

  // Read exit code from FIFO.
  let mut fifo_ret = File::open(&fifo_ret).context("failed to open exit code FIFO")?;
  let mut buf = Vec::new();
  let _cnt = fifo_ret
    .read_to_end(&mut buf)
    .context("failed to read from exit code FIFO")?;
  let rc = str::from_utf8(&buf)
    .context("failed to parse reported exit code: invalid string")?
    .trim_end()
    .parse::<i32>()
    .context("failed to parse reported exit code")?;
  exit(rc);
}


#[cfg(test)]
mod tests {
  use super::*;


  /// Make sure that we are able to
  #[test]
  fn tmux_env_var_filtering() {
    let tmux = OsStr::new("/tmp/tmux-1000/default,22830,9");
    let tmux = filter_tmux(tmux);
    assert_eq!(tmux, OsStr::new("/tmp/tmux-1000/default,22830"));

    let tmux = OsStr::new("/tmp/tmux-1000/default,22830");
    let tmux = filter_tmux(tmux);
    assert_eq!(tmux, OsStr::new("/tmp/tmux-1000/default,22830"));

    let tmux = OsStr::new("/tmp/tmux-1000/default");
    let tmux = filter_tmux(tmux);
    assert_eq!(tmux, OsStr::new("/tmp/tmux-1000/default"));
  }
}
