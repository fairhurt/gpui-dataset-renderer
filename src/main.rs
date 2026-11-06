#![allow(dead_code)]
// Release builds are a GUI app on Windows: no console window pops up next to the UI.
// Debug builds keep the console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod data;
mod explorer;
mod html;
mod html_render;
mod theme;
mod ui;

use std::path::PathBuf;

use gpui::{App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

const USAGE: &str = "\
gpui-dataset-renderer — requirements explorer

USAGE:
    gpui-dataset-renderer [--count N]       generate N synthetic XML requirements in memory (default 150000)
    gpui-dataset-renderer --dir <folder>    load every *.xml file in <folder>
    gpui-dataset-renderer --write <folder> [--count N]
                                            write the synthetic dataset to disk as one XML file per requirement";

/// A GUI-subsystem exe has no console. When launched from a terminal, reattach to it so
/// `--help`, `--write` progress and errors still print there. No-op when started from Explorer.
#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() {
    #[cfg(windows)]
    attach_parent_console();

    let mut count: u32 = 150_000;
    let mut dir: Option<PathBuf> = None;
    let mut write: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--count" | "-n" => {
                count = args.next().and_then(|v| v.replace('_', "").parse().ok()).unwrap_or_else(|| {
                    eprintln!("--count needs a number\n\n{USAGE}");
                    std::process::exit(2)
                })
            }
            "--dir" | "-d" => dir = args.next().map(PathBuf::from),
            "--write" | "-w" => write = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    if let Some(out) = write {
        println!("Writing {count} requirement files to {} …", out.display());
        if let Err(e) = data::write_synthetic(&out, count) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1640.), px(1000.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Requirements Explorer — GPUI".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(1280.), px(720.))),
                focus: true,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| app::ReqApp::new(app::Options { count, dir }, window, cx)),
        )
        .expect("failed to open window");
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
