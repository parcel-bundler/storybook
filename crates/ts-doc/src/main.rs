use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ts_doc::fs_graph::FsBundleGraph;
use ts_doc::package;

fn main() -> ExitCode {
  let mut args = std::env::args().skip(1);
  let Some(entry) = args.next().map(PathBuf::from) else {
    eprintln!("usage: ts-doc <entry-file> [project-root]");
    eprintln!("  Parses <entry-file> and its dependencies, then prints the packaged API JSON.");
    return ExitCode::FAILURE;
  };

  // Default the project root to the nearest ancestor containing node_modules,
  // falling back to the entry's directory.
  let project_root = args
    .next()
    .map(PathBuf::from)
    .unwrap_or_else(|| find_project_root(&entry));

  let timing = std::env::var("TS_DOC_TIMING").is_ok();
  let t0 = std::time::Instant::now();
  let graph = FsBundleGraph::build(&entry, &project_root);
  let t1 = std::time::Instant::now();
  let output = package(&graph);
  let t2 = std::time::Instant::now();
  if timing {
    eprintln!("build (parse+resolve): {:?}", t1 - t0);
    eprintln!("package:               {:?}", t2 - t1);
  }

  // Write directly to stdout instead of building the whole (potentially
  // multi-megabyte) JSON string in memory first.
  let stdout = std::io::stdout();
  let mut writer = std::io::BufWriter::new(stdout.lock());
  match serde_json::to_writer_pretty(&mut writer, &output) {
    Ok(()) => {
      use std::io::Write;
      let _ = writer.flush();
      ExitCode::SUCCESS
    }
    Err(err) => {
      eprintln!("failed to serialize output: {err}");
      ExitCode::FAILURE
    }
  }
}

fn find_project_root(entry: &Path) -> PathBuf {
  let mut dir = entry.parent();
  while let Some(d) = dir {
    if d.join("node_modules").is_dir() {
      return d.to_owned();
    }
    dir = d.parent();
  }
  entry.parent().unwrap_or(Path::new(".")).to_owned()
}
