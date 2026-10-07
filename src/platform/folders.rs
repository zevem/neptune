//! Native folder and file selection, parented to Neptune and awaited off the
//! UI thread.
use std::{path::PathBuf, sync::mpsc};

#[derive(Default)]
pub(crate) struct Picker {
    dialog: rfd::AsyncFileDialog,
}

impl Picker {
    pub(crate) fn new(parent: &eframe::CreationContext<'_>) -> Self {
        Self {
            dialog: rfd::AsyncFileDialog::new().set_parent(parent),
        }
    }

    pub(crate) fn open(
        &self,
        directory: Option<PathBuf>,
        wake: eframe::egui::Context,
    ) -> Result<mpsc::Receiver<Result<Option<PathBuf>, String>>, String> {
        let mut dialog = self.dialog.clone().set_title("Choose default directory");
        if let Some(directory) = directory {
            dialog = dialog.set_directory(directory);
        }
        // macOS requires creating the dialog on the app's main thread. The
        // future waits on a worker while the native event loop keeps running.
        let selection = dialog.pick_folder();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("neptune-folder-picker".into())
            .spawn(move || {
                let directory = pollster::block_on(selection).map(|folder| folder.path().into());
                let _ = sender.send(Ok(directory));
                wake.request_repaint();
            })
            .map_err(|error| format!("Cannot open the folder picker: {error}"))?;
        Ok(receiver)
    }

    /// Asks for files to attach to a message. Nothing chosen is no file.
    pub(crate) fn open_files(
        &self,
        wake: eframe::egui::Context,
    ) -> Result<mpsc::Receiver<Vec<PathBuf>>, String> {
        // As above: made on the main thread, awaited on a worker.
        let selection = self.dialog.clone().set_title("Attach files").pick_files();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("neptune-file-picker".into())
            .spawn(move || {
                let files = pollster::block_on(selection)
                    .unwrap_or_default()
                    .iter()
                    .map(|file| file.path().into())
                    .collect();
                let _ = sender.send(files);
                wake.request_repaint();
            })
            .map_err(|error| format!("Cannot open the file picker: {error}"))?;
        Ok(receiver)
    }
}
