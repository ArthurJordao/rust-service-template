use std::sync::Arc;

/// Per-subscriber processing knobs. Returned by `Subscriber::consumer_config`.
/// `concurrency: 1` means message-by-message (serial, in `id` order).
#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    pub batch_size: i64,
    pub concurrency: usize,
    pub poll_interval: std::time::Duration,
}

impl Default for ConsumerConfig {
    fn default() -> Self {
        ConsumerConfig {
            batch_size: 10,
            concurrency: 5,
            poll_interval: std::time::Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewEvent {
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub correlation_id: String,
}

#[derive(Debug, Clone)]
pub struct DeliveredEvent {
    pub event_id: i64,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: serde_json::Value,
    pub correlation_id: String,
}

/// A consumer of a single event type. Concrete implementations hold whatever
/// domain dependencies (repositories, publishers) they need.
#[async_trait::async_trait]
pub trait Subscriber: Send + Sync {
    fn name(&self) -> &'static str;
    fn event_type(&self) -> &'static str;
    async fn handle(&self, event: &DeliveredEvent) -> anyhow::Result<()>;
    /// How this subscriber's consumer loop claims and processes deliveries.
    /// Override to opt into message-by-message (`concurrency: 1`) or to tune
    /// batch size / poll cadence. Defaults are batched-and-concurrent.
    fn consumer_config(&self) -> ConsumerConfig {
        ConsumerConfig::default()
    }
}

#[derive(Default)]
pub struct SubscriberRegistry {
    subscribers: Vec<Arc<dyn Subscriber>>,
}

impl SubscriberRegistry {
    pub fn new() -> Self {
        SubscriberRegistry {
            subscribers: Vec::new(),
        }
    }

    pub fn register(&mut self, s: Arc<dyn Subscriber>) {
        self.subscribers.push(s);
    }

    /// Names of all subscribers interested in `event_type` (drives fan-out).
    pub fn names_for(&self, event_type: &str) -> Vec<&'static str> {
        self.subscribers
            .iter()
            .filter(|s| s.event_type() == event_type)
            .map(|s| s.name())
            .collect()
    }

    pub fn find(&self, name: &str) -> Option<Arc<dyn Subscriber>> {
        self.subscribers.iter().find(|s| s.name() == name).cloned()
    }
}

/// Event-type -> subscriber-name routing table used by the publisher to fan out
/// delivery rows. Plain data (no subscriber instances) so the publisher never
/// depends on the subscribers it routes to.
#[derive(Debug, Clone, Default)]
pub struct Routes {
    map: std::collections::HashMap<String, Vec<String>>,
}

impl Routes {
    pub fn new() -> Self {
        Routes::default()
    }

    pub fn add(mut self, event_type: &str, subscriber_name: &str) -> Self {
        self.map
            .entry(event_type.to_string())
            .or_default()
            .push(subscriber_name.to_string());
        self
    }

    pub fn names_for(&self, event_type: &str) -> Vec<String> {
        self.map.get(event_type).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;
    #[async_trait::async_trait]
    impl Subscriber for Dummy {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn event_type(&self) -> &'static str {
            "thing.happened"
        }
        async fn handle(&self, _e: &DeliveredEvent) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn registry_finds_subscribers_by_event_type_and_name() {
        let mut reg = SubscriberRegistry::new();
        reg.register(Arc::new(Dummy));
        assert_eq!(reg.names_for("thing.happened"), vec!["dummy"]);
        assert!(reg.names_for("other").is_empty());
        assert!(reg.find("dummy").is_some());
        assert!(reg.find("missing").is_none());
    }

    #[test]
    fn routes_maps_event_types_to_subscriber_names() {
        let routes = Routes::new()
            .add("user.registered", "account.on-user-registered")
            .add("user.registered", "audit.log");
        assert_eq!(
            routes.names_for("user.registered"),
            vec![
                "account.on-user-registered".to_string(),
                "audit.log".to_string()
            ]
        );
        assert!(routes.names_for("other").is_empty());
    }

    #[test]
    fn consumer_config_defaults_and_override() {
        let d = ConsumerConfig::default();
        assert_eq!(d.batch_size, 10);
        assert_eq!(d.concurrency, 5);
        assert_eq!(d.poll_interval, std::time::Duration::from_secs(2));

        // A subscriber inherits the default unless it overrides.
        assert_eq!(Dummy.consumer_config().concurrency, 5);
    }
}
