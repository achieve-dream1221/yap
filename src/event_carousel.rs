//! The "Event Carousel" is a simple actor that takes in closures, and calls them with a delay/on an interval.
//!
//! It's initial purpose was to let me trigger immediate-mode UI updates with a specific delay by sending an event to the main+UI thread,
//! but it also became useful as a way to delay/schedule other app actions.
//!
//! For example, one closure supplied is set to send Tick::PerSecond once per second,
//! which is used to trigger reconnection attempts + flip the repeating line widget for a visual indicator for such,
//! among other things.

// TODO i think we can go back to just events to main+UI rather than needing closures

use std::{
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crossbeam::channel::{Receiver, RecvTimeoutError, Sender, bounded};
use tracing::{debug, error, warn};

enum CarouselCommand {
    AddEvent(CarouselClosure),
    Shutdown(Sender<()>),
}

pub struct CarouselHandle {
    command_tx: Sender<CarouselCommand>,
}

type HandleResult<T> = Result<T, CarouselWorkerMissing>;

type PayloadFn = Box<dyn Fn() -> Result<(), String> + Send + 'static>;

#[derive(Debug, thiserror::Error)]
#[error("carousel rx handle dropped")]
pub struct CarouselWorkerMissing;

impl<T> From<crossbeam::channel::SendError<T>> for CarouselWorkerMissing {
    fn from(_: crossbeam::channel::SendError<T>) -> Self {
        Self
    }
}

// impl Drop for CarouselHandle {}

impl CarouselHandle {
    pub fn new() -> (Self, JoinHandle<()>) {
        let (command_tx, command_rx) = bounded(20);

        let mut worker = CarouselWorker {
            // event_tx,
            command_rx,
            closures: Vec::new(),
            last_woke_at: Instant::now(),
        };

        let worker = std::thread::spawn(move || {
            if let Err(e) = worker.work_loop() {
                error!("Carousel worker closed with error: {e}");
            } else {
                debug!("Carousel worker closed gracefully!");
            }
        });

        (Self { command_tx }, worker)
    }
    /// Supply a closure to be called ad-infinitum at a specified interval.
    pub fn add_repeating<S: Into<String>>(
        &self,
        name: S,
        payload: PayloadFn,
        interval: Duration,
    ) -> HandleResult<()> {
        let event = CarouselClosure {
            name: name.into(),
            payload,
            interval,
            last_sent: Instant::now(),
            event_type: EventType::Repeating,
        };
        self.command_tx.send(CarouselCommand::AddEvent(event))?;
        Ok(())
    }
    /// Supply a closure to be called after a specified delay,
    /// calling again with the same name will "reset" the delay.
    pub fn add_oneshot<S: Into<String>>(
        &self,
        name: S,
        payload: PayloadFn,
        delay: Duration,
    ) -> HandleResult<()> {
        let event = CarouselClosure {
            name: name.into(),
            payload,
            interval: delay,
            last_sent: Instant::now(),
            event_type: EventType::Oneshot,
        };
        self.command_tx.send(CarouselCommand::AddEvent(event))?;
        Ok(())
    }
    pub fn shutdown(&self) -> Result<(), ()> {
        let (shutdown_tx, shutdown_rx) = bounded(0);
        if self
            .command_tx
            .send(CarouselCommand::Shutdown(shutdown_tx))
            .is_ok()
        {
            if shutdown_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
                Ok(())
            } else {
                error!("Carousel didn't react to shutdown request.");
                Err(())
            }
        } else {
            error!("Couldn't send carousel shutdown.");
            Err(())
        }
    }
}

struct CarouselClosure {
    name: String,
    interval: Duration,
    payload: PayloadFn,
    last_sent: Instant,
    event_type: EventType,
}

#[derive(PartialEq, Eq)]
enum EventType {
    Repeating,
    Oneshot,
    ExpiredOneshot,
}

/// Worker that sleeps until it's time to call a closure.
struct CarouselWorker {
    command_rx: Receiver<CarouselCommand>,
    last_woke_at: Instant,
    closures: Vec<CarouselClosure>,
}

impl CarouselWorker {
    fn work_loop(&mut self) -> Result<(), CarouselWorkerError> {
        let mut sleep_time = Duration::from_secs(5);
        let mut closure_had_error = false;
        loop {
            match self.command_rx.recv_timeout(sleep_time) {
                Ok(CarouselCommand::AddEvent(event)) => {
                    // Replace any event with the same name if one is present.
                    if let Some(index) = self.closures.iter().position(|ev| ev.name == event.name) {
                        self.closures.remove(index);
                    }
                    self.closures.push(event);
                }
                Ok(CarouselCommand::Shutdown(shutdown_tx)) => {
                    if shutdown_tx.send(()).is_err() {
                        error!("Failed to reply to shutdown request!");
                        return Err(CarouselWorkerError::ShutdownReply);
                    } else {
                        break Ok(());
                    }
                }
                Err(RecvTimeoutError::Timeout) => (),
                Err(RecvTimeoutError::Disconnected) => {
                    warn!("Handle dropped, closing carousel thread!");
                    return Err(CarouselWorkerError::HandleDropped);
                }
            };
            let now = Instant::now();
            self.last_woke_at = now;

            sleep_time = self
                .closures
                .iter_mut()
                .fold(Duration::from_secs(5), |shortest, ev| {
                    let since_last_send = now.duration_since(ev.last_sent);

                    // If we've run longer than the interval
                    let until_next_send = if since_last_send >= ev.interval {
                        ev.last_sent = now;

                        // info!("meow! {:?}", e.interval);
                        if let Err(err) = (ev.payload)() {
                            error!(
                                "Event {name}'s payload had error `{err}`, closing carousel thread.",
                                name = ev.name
                            );
                            closure_had_error = true;
                        }

                        if ev.event_type == EventType::Oneshot {
                            ev.event_type = EventType::ExpiredOneshot;
                        }

                        ev.interval
                    } else {
                        // get the remaining amount of time in our sleep budget
                        ev.interval - since_last_send
                    };

                    shortest.min(until_next_send)
                });

            if closure_had_error {
                break Err(CarouselWorkerError::EventTrigger);
            }

            // Removing any expired oneshots
            self.closures
                .retain(|e| e.event_type != EventType::ExpiredOneshot);
            // info!("Waiting for {sleep_time:?}");
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CarouselWorkerError {
    #[error("closure returned error")]
    EventTrigger,
    #[error("failed to reply to shutdown request in time")]
    ShutdownReply,
    #[error("handle dropped, can't recieve commands")]
    HandleDropped,
}
