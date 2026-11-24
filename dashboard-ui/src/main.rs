use std::path::PathBuf;

use carfluxoid_dashboard_ui::{render_screenshots, run_daemon, state::Paths};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut paths = Paths::default();
    let mut screenshot_dir: Option<PathBuf> = None;
    let mut logo_path: Option<PathBuf> = None;
    let mut preview = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--settings" if i + 1 < args.len() => {
                paths.settings = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--assets" if i + 1 < args.len() => {
                paths.assets = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--runtime" if i + 1 < args.len() => {
                paths.runtime = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--bridge" if i + 1 < args.len() => {
                paths.bridge = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--screenshot" if i + 1 < args.len() => {
                screenshot_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--logo" if i + 1 < args.len() => {
                logo_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--preview" => {
                preview = true;
                i += 1;
            }
            other => {
                eprintln!("Unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    if let Some(dir) = screenshot_dir {
        if let Err(err) = render_screenshots(&dir, logo_path.as_deref()) {
            eprintln!("Screenshot failed: {err}");
            std::process::exit(1);
        }
        return;
    }

    if preview {
        #[cfg(feature = "preview")]
        {
            if let Err(err) = carfluxoid_dashboard_ui::run_preview(logo_path.as_deref()) {
                eprintln!("Preview failed: {err}");
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(feature = "preview"))]
        {
            eprintln!("Rebuild with `--features preview` to open the desktop window.");
            std::process::exit(1);
        }
    }

    if let Err(err) = run_daemon(paths) {
        eprintln!("CarFluxoid dashboard error: {err}");
        std::process::exit(1);
    }
}
