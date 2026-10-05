#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use neptune_terminal::{Launch, app, config, persistence::window_state};

fn main() -> anyhow::Result<()> {
    // SAFETY: nothing has started a thread yet.
    #[cfg(target_os = "linux")]
    unsafe {
        neptune_terminal::platform::host_env::restore()
    };
    if let Some(code) =
        neptune_terminal::runtime::agents::cli(&std::env::args().skip(1).collect::<Vec<_>>())?
    {
        std::process::exit(code);
    }
    let mut launch = Launch::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cwd" => {
                launch.cwd = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--cwd needs a path"))?
                        .into(),
                )
            }
            "--ssh" => {
                let destination = args.next().ok_or_else(|| {
                    anyhow::anyhow!("--ssh needs a destination such as user@host")
                })?;
                neptune_model::Remote::parse(&destination)
                    .map_err(|error| anyhow::anyhow!("--ssh {destination:?}: {error}"))?;
                launch.ssh = Some(destination);
            }
            "--config" => {
                launch.config = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--config needs a path"))?
                        .into(),
                )
            }
            "--data-root" => {
                launch.data_root = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--data-root needs a path"))?
                        .into(),
                );
            }
            "--command" => {
                launch.command = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--command needs a shell command"))?,
                )
            }
            "--screenshot" => {
                launch.screenshot = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--screenshot needs a path"))?
                        .into(),
                )
            }
            "--size" => {
                let size = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--size needs WIDTHxHEIGHT"))?;
                let (w, h) = size
                    .split_once('x')
                    .ok_or_else(|| anyhow::anyhow!("--size needs WIDTHxHEIGHT"))?;
                let w: f32 = w.parse()?;
                let h: f32 = h.parse()?;
                anyhow::ensure!(
                    window_state::WindowState::valid_size([w, h]),
                    "size must be 640x400 to 8192x8192"
                );
                launch.size = Some([w, h]);
            }
            "--no-restore" => launch.no_restore = true,
            "--diagnostics" => launch.diagnostics = true,
            "--version" | "-V" => {
                println!("Neptune {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "Neptune — a native GPU terminal\n\nUsage: neptune [OPTIONS]\n  --cwd PATH         Open a workspace at PATH\n  --ssh DESTINATION  Open a workspace whose terminals run on an SSH host\n  --config PATH      Use a TOML configuration\n  --data-root PATH   Isolate settings and saved workspace/window state\n  --command COMMAND  Run a command in the first terminal\n  --no-restore       Start without saved workspaces\n  --size WIDTHxHEIGHT Override saved window size and maximized state\n  --screenshot PATH  Capture the native window after 3 seconds and exit\n  --diagnostics      Print renderer and display details\n  --version\n  --help"
                );
                return Ok(());
            }
            _ => anyhow::bail!("Unknown option {arg}. Try --help"),
        }
    }
    if let Some(cwd) = &launch.cwd {
        anyhow::ensure!(cwd.is_dir(), "Directory does not exist: {}", cwd.display());
    }
    // A command is typed into the first terminal shortly after it starts. Over
    // SSH that could be a password or host-key prompt rather than a shell.
    anyhow::ensure!(
        launch.ssh.is_none() || launch.command.is_none(),
        "--command cannot be combined with --ssh"
    );
    let data_root = launch.data_root.clone();
    let ephemeral = launch.screenshot.is_some();
    // Resolve/migrate storage and read geometry before creating the window.
    // Failure must preserve old work rather than start saving into a new root.
    let (data, mut window) = std::thread::Builder::new()
        .name("neptune-window-restore".into())
        .spawn(move || -> anyhow::Result<_> {
            let data = match data_root {
                Some(data) => data,
                None if ephemeral => config::data_dir(),
                None => config::prepare_data_dir()?,
            };
            let window = if ephemeral {
                window_state::LoadReport::default()
            } else {
                window_state::load(&data.join("window.json"))
            };
            Ok((data, window))
        })?
        .join()
        .map_err(|_| {
            anyhow::anyhow!("Storage restoration worker stopped; saved data is preserved")
        })??;
    // An update restarts into the same storage, not the one-time options.
    for (option, path) in [
        ("--config", &launch.config),
        ("--data-root", &launch.data_root),
    ] {
        if let Some(path) = path {
            launch.relaunch_arguments.push(option.into());
            launch.relaunch_arguments.push(
                std::path::absolute(path)
                    .unwrap_or_else(|_| path.clone())
                    .into(),
            );
        }
    }
    launch.data_root = Some(data);
    if let Some(size) = launch.size {
        window.state.inner_size = size;
        window.state.maximized = false;
    }
    eframe::run_native(
        "Neptune",
        native_options(&window.state, native_icon()?),
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, launch, window)))),
    )
    .map_err(|e| anyhow::anyhow!("Cannot start native renderer: {e}"))
}

fn native_options(
    window: &window_state::WindowState,
    icon: eframe::egui::IconData,
) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Neptune")
            .with_app_id("rs.neptune.terminal")
            .with_icon(icon)
            .with_inner_size(window.inner_size)
            .with_maximized(window.maximized)
            .with_min_inner_size([640.0, 400.0])
            // Windows rounds and borders the window itself; elsewhere the
            // application paints transparent corners.
            .with_transparent(!cfg!(target_os = "windows"))
            .with_decorations(false),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration::default().with_surface_config(
            eframe::egui_wgpu::SurfaceConfig {
                // NVIDIA's FIFO acquire can block the UI thread for seconds
                // after an idle/covered window on Linux hybrid graphics. The
                // automatic mode uses Immediate/Mailbox where supported and
                // retains FIFO as a fallback, without assuming Mailbox exists.
                present_mode: if cfg!(target_os = "linux") {
                    eframe::wgpu::PresentMode::AutoNoVsync
                } else {
                    eframe::wgpu::PresentMode::AutoVsync
                },
                ..eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY
            },
        ),
        ..Default::default()
    }
}

fn native_icon() -> anyhow::Result<eframe::egui::IconData> {
    // eframe replaces the macOS Dock icon at launch. Use the same padded artwork
    // as the app bundle so the icon keeps its Finder/closed-app size.
    let png: &[u8] = if cfg!(target_os = "macos") {
        include_bytes!("../assets/branding/neptune-macos-logo.png")
    } else {
        include_bytes!("../assets/icons/neptune-256.png")
    };
    eframe::icon_data::from_png_bytes(png)
        .map_err(|error| anyhow::anyhow!("Cannot load bundled Neptune icon: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_window_supports_transparent_corners() {
        let options = native_options(
            &window_state::WindowState::default(),
            native_icon().expect("bundled icon decodes"),
        );
        assert_eq!(
            options.viewport.transparent,
            Some(!cfg!(target_os = "windows"))
        );
        assert_eq!(options.viewport.decorations, Some(false));
    }

    #[test]
    fn bundled_logo_has_transparent_padding_and_opaque_artwork() {
        let icon = native_icon().expect("bundled icon decodes");
        let size = if cfg!(target_os = "macos") { 1024 } else { 256 };
        assert_eq!((icon.width, icon.height), (size, size));
        assert_eq!(icon.rgba.len(), (size * size * 4) as usize);
        assert_eq!(icon.rgba[3], 0);
        let center = (size / 2 * size + size / 2) as usize;
        assert_eq!(icon.rgba[center * 4 + 3], 255);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn runtime_dock_icon_preserves_macos_bundle_padding() {
        let icon = native_icon().expect("bundled icon decodes");
        let mut bounds = [icon.width, icon.height, 0, 0];
        for (index, pixel) in icon.rgba.chunks_exact(4).enumerate() {
            if pixel[3] > 0 {
                let x = index as u32 % icon.width;
                let y = index as u32 / icon.width;
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x + 1);
                bounds[3] = bounds[3].max(y + 1);
            }
        }
        assert_eq!(bounds, [100, 100, 924, 924]);
    }
}
