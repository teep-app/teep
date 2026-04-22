use std::time::Duration;

use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    time::{MissedTickBehavior, interval},
};
use tracing::warn;

use crate::app::Msg;

/// Unified event loop: terminal events, periodic ticks, fs-watch, and job
/// results all funnel into a single `Msg` channel. The main loop pulls
/// `Msg`s out in arrival order; additional sources attach by calling
/// `sender()` and sending their own `Msg`s.
pub struct EventLoop {
    rx: UnboundedReceiver<Msg>,
    tx: UnboundedSender<Msg>,
}

impl EventLoop {
    pub fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        tokio::spawn(terminal_loop(tx.clone()));
        tokio::spawn(tick_loop(tx.clone()));
        Self { rx, tx }
    }

    pub fn sender(&self) -> UnboundedSender<Msg> {
        self.tx.clone()
    }

    pub async fn next(&mut self) -> Option<Msg> {
        self.rx.recv().await
    }
}

impl Default for EventLoop {
    fn default() -> Self {
        Self::new()
    }
}

async fn terminal_loop(tx: UnboundedSender<Msg>) {
    let mut stream = EventStream::new();
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(ct_ev) => {
                if let Some(msg) = map_terminal_event(ct_ev)
                    && tx.send(msg).is_err()
                {
                    return;
                }
            }
            Err(e) => {
                warn!(?e, "terminal event error");
            }
        }
    }
}

async fn tick_loop(tx: UnboundedSender<Msg>) {
    let mut tick = interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        if tx.send(Msg::Tick).is_err() {
            return;
        }
    }
}

fn map_terminal_event(ev: CtEvent) -> Option<Msg> {
    match ev {
        CtEvent::Key(k) if k.kind == KeyEventKind::Press => Some(Msg::Key(k)),
        CtEvent::Mouse(m) => Some(Msg::Mouse(m)),
        CtEvent::Resize(w, h) => Some(Msg::Resize(w, h)),
        CtEvent::Key(_) | CtEvent::FocusGained | CtEvent::FocusLost | CtEvent::Paste(_) => None,
    }
}
