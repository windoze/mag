//! In-memory event fan-out used by transport front doors.

use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};

use mag_service::Event;
use tokio::sync::broadcast;
use tokio_stream::{
    Stream,
    wrappers::{BroadcastStream, errors::BroadcastStreamRecvError},
};

const DEFAULT_EVENT_CAPACITY: usize = 1024;

/// In-memory broadcast bus for engine events.
#[derive(Clone, Debug)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl EventBus {
    /// Creates an event bus with the default channel capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_EVENT_CAPACITY)
    }

    /// Creates an event bus with a caller-supplied channel capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    /// Subscribes to future events emitted on this bus.
    #[must_use]
    pub fn subscribe(&self) -> EventStream {
        EventStream {
            inner: BroadcastStream::new(self.sender.subscribe()),
        }
    }

    /// Broadcasts an event to all active subscribers.
    ///
    /// The returned value is the number of receivers that accepted the event.
    /// Emitting with no active subscribers is treated as a successful no-op.
    #[must_use]
    pub fn emit(&self, event: Event) -> usize {
        self.sender.send(event).unwrap_or(0)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Stream of future events from an [`EventBus`].
pub struct EventStream {
    inner: BroadcastStream<Event>,
}

impl Stream for EventStream {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            match ready!(Pin::new(&mut this.inner).poll_next(cx)) {
                Some(Ok(event)) => return Poll::Ready(Some(event)),
                Some(Err(BroadcastStreamRecvError::Lagged(_))) => continue,
                None => return Poll::Ready(None),
            }
        }
    }
}
