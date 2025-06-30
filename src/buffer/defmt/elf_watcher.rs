use std::{
    path::Path,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use crossbeam::channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, bounded};
use notify::{
    EventKind, RecommendedWatcher, Watcher,
    event::{ModifyKind, RenameMode},
};
use tracing::{debug, error, info, trace};

use crate::app::Event;

#[derive(Debug)]
pub enum ElfWatchEvent {
    ElfUpdated(Utf8PathBuf),
    Error(String),
}

enum ElfWatchCommand {
    BeginWatch(Utf8PathBuf),
    EndWatch,
    Shutdown(Sender<()>),
}

pub struct ElfWatchHandle {
    command_tx: Sender<ElfWatchCommand>,
}

// TODO make a bespoke error type to use around the work loop

impl ElfWatchHandle {
    pub fn build(event_tx: Sender<Event>) -> Result<(Self, JoinHandle<()>), notify::Error> {
        let (command_tx, command_rx) = bounded(1);
        let (watcher_tx, watcher_rx) = bounded(10);

        let watcher = notify::recommended_watcher(watcher_tx)?;

        let mut worker = ElfWatchWorker {
            command_rx,
            event_tx,
            watcher_rx,
            watcher,
            file_under_watch: None,
            load_debounce_instant: Instant::now(),
        };

        let worker = std::thread::spawn(move || {
            worker
                .work_loop()
                .expect("ELF Watcher encountered a fatal error");
        });

        Ok((Self { command_tx }, worker))
    }

    pub fn begin_watch(&self, elf_path: &Utf8Path) {
        let path = elf_path.to_owned();
        self.command_tx
            .send(ElfWatchCommand::BeginWatch(path))
            .unwrap();
    }

    pub fn end_watch(&self) {
        self.command_tx.send(ElfWatchCommand::EndWatch).unwrap();
    }

    pub fn shutdown(&self) -> Result<(), ()> {
        let (shutdown_tx, shutdown_rx) = bounded(0);
        if self
            .command_tx
            .send(ElfWatchCommand::Shutdown(shutdown_tx))
            .is_ok()
        {
            if shutdown_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
                Ok(())
            } else {
                error!("ELF watcher thread didn't react to shutdown request.");
                Err(())
            }
        } else {
            error!("Couldn't send ELF watcher shutdown.");
            Err(())
        }
    }
}

const DEBOUNCE_DURATION: Duration = Duration::from_secs(2);

struct ElfWatchWorker {
    command_rx: Receiver<ElfWatchCommand>,
    event_tx: Sender<Event>,
    watcher: RecommendedWatcher,
    watcher_rx: Receiver<Result<notify::Event, notify::Error>>,
    file_under_watch: Option<Utf8PathBuf>,
    load_debounce_instant: Instant,
}

impl ElfWatchWorker {
    pub fn work_loop(&mut self) -> Result<(), std::io::Error> {
        loop {
            // if let Ok(watcher_event_res) =

            let mut channel_notifier = crossbeam::channel::Select::new();
            channel_notifier.recv(&self.watcher_rx);
            channel_notifier.recv(&self.command_rx);
            // Waiting...
            let _ready_index = channel_notifier.ready();

            match self.watcher_rx.try_recv() {
                Ok(watcher_event_res) => {
                    match watcher_event_res {
                        Ok(watcher_event) if self.event_matches_watched_file(&watcher_event) => {
                            // trace!("File watcher event: {watcher_event:?}");

                            if self.load_debounce_instant.elapsed() < DEBOUNCE_DURATION {
                                info!("Ignoring ELF file update, too soon since last one.");
                            } else {
                                let owned_watched_path =
                                    self.file_under_watch.as_ref().unwrap().to_owned();

                                if let Err(e) = self.event_tx.send(Event::DefmtElfWatch(
                                    ElfWatchEvent::ElfUpdated(owned_watched_path),
                                )) {
                                    error!("Error sending file watch event, stopping thread: {e}");
                                    break;
                                }
                                self.load_debounce_instant = Instant::now();
                                debug!("ELF Watcher sent reload request.");
                            }
                        }
                        Ok(_) => (),
                        Err(e) => error!("File watcher error: {e}"),
                    }
                }
                Err(TryRecvError::Empty) => (),
                Err(TryRecvError::Disconnected) => break,
            }

            match self.command_rx.try_recv() {
                Ok(ElfWatchCommand::Shutdown(shutdown_tx)) => {
                    shutdown_tx
                        .send(())
                        .expect("Failed to reply to shutdown request");
                    break;
                }
                Ok(command) => {
                    self.handle_command(command).unwrap();
                }
                Err(TryRecvError::Empty) => (),
                Err(TryRecvError::Disconnected) => break,
            }
        }

        Ok(())
    }
    fn event_matches_watched_file(&self, event: &notify::Event) -> bool {
        if let Some(watched_path) = &self.file_under_watch
            && event.paths.iter().any(|p| p == watched_path)
        {
            // guh.
            match event.kind {
                EventKind::Create(_) => true,
                EventKind::Modify(modify_kind) => match modify_kind {
                    ModifyKind::Data(_) => true,
                    ModifyKind::Any => true,
                    ModifyKind::Other => true,
                    ModifyKind::Metadata(_) => false,
                    ModifyKind::Name(rename_mode) => match rename_mode {
                        RenameMode::To => true,
                        RenameMode::From => false,

                        RenameMode::Both if event.paths[1] == *watched_path => true,
                        RenameMode::Both => false,

                        RenameMode::Any | RenameMode::Other => true,
                    },
                },
                EventKind::Any => true,
                EventKind::Other => true,
                EventKind::Access(_) => false,
                EventKind::Remove(_) => false,
            }
        } else {
            false
        }
    }
    fn handle_command(&mut self, command: ElfWatchCommand) -> Result<(), std::io::Error> {
        match command {
            ElfWatchCommand::BeginWatch(new_file) => {
                info!("Asked to watch for updates to: {new_file}");
                // Check if we're already watching it
                if let Some(current_path) = &self.file_under_watch
                    && *current_path == new_file
                {
                    info!("Already watching! Not acting further.");
                    return Ok(());
                }

                let new_file_parent = new_file.parent().ok_or("file has no parent").unwrap();

                self.handle_command(ElfWatchCommand::EndWatch)?;

                if let Err(e) = self.watcher.watch(
                    new_file_parent.as_ref(),
                    notify::RecursiveMode::NonRecursive,
                ) {
                    self.event_tx
                        .send(Event::DefmtElfWatch(ElfWatchEvent::Error(e.to_string())))
                        .unwrap();
                }
                _ = self.file_under_watch.insert(new_file);
            }
            ElfWatchCommand::EndWatch => {
                if let Some(old_path) = self.file_under_watch.take() {
                    let old_path_parent = old_path.parent().unwrap();
                    if let Err(e) = self.watcher.unwatch(old_path_parent.as_ref()) {
                        error!("Error unwatching file: {e}")
                    }
                }
            }
            ElfWatchCommand::Shutdown(_) => unreachable!(),
        }
        Ok(())
    }
}
