use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TrySendError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use tracing::{error, info};

enum CarouselCommand<T> {
    AddEvent(T, Duration),
    Shutdown(Sender<()>),
}

// #[derive(Clone)]
// Could maybe remove Clone (and maybe Send) if I use Box<Fn() -> T> instead
pub struct CarouselHandle<T: Clone + Send + std::fmt::Debug> {
    command_tx: Sender<CarouselCommand<T>>,
}

impl<T: Clone + Send + std::fmt::Debug + 'static> CarouselHandle<T> {
    pub fn new(event_tx: Sender<T>) -> (Self, JoinHandle<()>) {
        let (command_tx, command_rx) = mpsc::channel();

        let mut worker = CarouselWorker {
            event_tx,
            command_rx,
            events: Vec::new(),
            woke_at: Instant::now(),
        };

        let worker = std::thread::spawn(move || {
            worker
                .work_loop()
                .expect("Carousel encountered a fatal error");
        });

        (Self { command_tx }, worker)
    }
    pub fn add_event(&self, payload: T, interval: Duration) {
        self.command_tx
            .send(CarouselCommand::AddEvent(payload, interval))
            .unwrap();
    }
    pub fn shutdown(&self) -> Result<(), ()> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
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

struct CarouselEvent<T: Clone> {
    interval: Duration,
    payload: T,
    last_sent: Instant,
}

struct CarouselWorker<T: Clone> {
    event_tx: Sender<T>,
    command_rx: Receiver<CarouselCommand<T>>,
    woke_at: Instant,
    // Could also be a Box<Fn> maybe
    events: Vec<CarouselEvent<T>>,
}

impl<T: Clone> CarouselWorker<T> {
    fn work_loop(&mut self) -> Result<(), ()> {
        let mut sleep_time = Duration::from_secs(5);
        let mut send_error = false;
        loop {
            let now = Instant::now();
            match self.command_rx.recv_timeout(sleep_time) {
                Ok(CarouselCommand::AddEvent(payload, interval)) => {
                    let event = CarouselEvent {
                        payload,
                        interval,
                        last_sent: now,
                    };
                    self.events.push(event);
                }
                Ok(CarouselCommand::Shutdown(shutdown_tx)) => {
                    shutdown_tx
                        .send(())
                        .expect("Failed to reply to shutdown request");
                    break;
                }
                Err(RecvTimeoutError::Timeout) => (),
                Err(RecvTimeoutError::Disconnected) => panic!("Carousel lost all Handles"),
            };
            let now = Instant::now();
            self.woke_at = now;

            sleep_time = self
                .events
                .iter_mut()
                .fold(Duration::from_secs(5), |shortest, e| {
                    let since_last_send = now.duration_since(e.last_sent);

                    // If we've run longer than the interval
                    let until_next_send = if since_last_send >= e.interval {
                        e.last_sent = now;

                        // info!("meow! {:?}", e.interval);
                        let payload = e.payload.clone();
                        if let Err(e) = self.event_tx.send(payload) {
                            error!("Failed to send {e:?}, closing carousel thread.");
                            send_error = true;
                        }
                        e.interval
                    } else {
                        let remaining = e.interval - since_last_send;
                        remaining
                    };

                    shortest.min(until_next_send)
                });

            if send_error {
                break;
            }

            // info!("Waiting for {sleep_time:?}");
        }
        if send_error {
            Err(())
        } else {
            Ok(())
        }
    }
}
