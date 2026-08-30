use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

use crate::admin::contract::{AdminEvent, AdminEventEnvelope, LogLevel, LogRecord};

pub struct AdminHub {
    sequence: AtomicU64,
    log_capacity: usize,
    logs: Mutex<VecDeque<LogRecord>>,
    sender: broadcast::Sender<AdminEventEnvelope>,
}

impl AdminHub {
    pub fn new(log_capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(1_024);
        Self {
            sequence: AtomicU64::new(0),
            log_capacity,
            logs: Mutex::new(VecDeque::with_capacity(log_capacity)),
            sender,
        }
    }

    pub fn log_layer(self: &Arc<Self>) -> AdminLogLayer {
        AdminLogLayer {
            hub: Arc::clone(self),
        }
    }

    pub(crate) fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    pub(crate) fn logs(&self) -> Vec<LogRecord> {
        self.lock_logs().iter().cloned().collect()
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AdminEventEnvelope> {
        self.sender.subscribe()
    }

    pub(crate) fn publish(&self, event: AdminEvent) -> u64 {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        let envelope = AdminEventEnvelope { sequence, event };
        let _receivers = self.sender.send(envelope);
        sequence
    }

    fn publish_log(&self, mut record: LogRecord) {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        record.sequence = sequence;
        {
            let mut logs = self.lock_logs();
            if logs.len() == self.log_capacity {
                logs.pop_front();
            }
            logs.push_back(record.clone());
        }
        let _receivers = self.sender.send(AdminEventEnvelope {
            sequence,
            event: AdminEvent::Log(record),
        });
    }

    fn lock_logs(&self) -> MutexGuard<'_, VecDeque<LogRecord>> {
        self.logs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone)]
pub struct AdminLogLayer {
    hub: Arc<AdminHub>,
}

impl<S> Layer<S> for AdminLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warning,
            Level::INFO => LogLevel::Info,
            Level::DEBUG | Level::TRACE => LogLevel::Debug,
        };
        let mut visitor = LogFieldVisitor::default();
        event.record(&mut visitor);
        let now = jiff::Zoned::now();
        let timestamp = format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second());
        self.hub.publish_log(LogRecord {
            sequence: 0,
            timestamp,
            level,
            target: metadata.target().to_owned(),
            message: visitor.finish(),
        });
    }
}

#[derive(Default)]
struct LogFieldVisitor {
    message: Option<String>,
    fields: Vec<String>,
}

impl LogFieldVisitor {
    fn finish(self) -> String {
        match (self.message, self.fields.is_empty()) {
            (Some(message), true) => message,
            (Some(message), false) => format!("{message} {}", self.fields.join(" ")),
            (None, _) => self.fields.join(" "),
        }
    }

    fn record_value(&mut self, field: &Field, value: String) {
        if field.name() == "message" {
            self.message = Some(value);
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }
}

impl Visit for LogFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record_value(field, format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field, value.to_owned());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field, value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field, value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field, value.to_string());
    }
}
