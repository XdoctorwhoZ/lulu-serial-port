//! Lightweight MQTT client wrapper built on top of [`rumqttc`].
//!
//! This module provides the thin MQTT integration that `lulu-serial-port` uses
//! to publish serial-port traffic to an MQTT broker.  The topic layout follows
//! the **lulu-logs** convention (`lulu/{source}/{attribute}`) so that messages
//! are consumable by any lulu-logs–compatible viewer.

use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};

// ---------------------------------------------------------------------------
// MqttConfig
// ---------------------------------------------------------------------------

/// Configuration for the MQTT connection used by [`crate::SerialPortManager`].
///
/// # Examples
///
/// ```
/// use lulu_serial_port::MqttConfig;
///
/// let config = MqttConfig {
///     broker_host: "192.168.1.10".to_string(),
///     broker_port: 1883,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct MqttConfig {
    /// MQTT broker hostname or IP address.
    pub broker_host: String,

    /// MQTT broker TCP port.
    pub broker_port: u16,

    /// Prefix used to generate the MQTT client ID (`{prefix}-{random}`).
    pub client_id_prefix: String,

    /// MQTT keep-alive interval in seconds.
    pub keep_alive_secs: u64,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            broker_host: "127.0.0.1".to_string(),
            broker_port: 1883,
            client_id_prefix: "lulu-serial-port".to_string(),
            keep_alive_secs: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// MqttHandle (crate-internal)
// ---------------------------------------------------------------------------

/// A lightweight, cloneable handle to a connected MQTT client.
///
/// Publishing is best-effort: errors are logged at `warn` level but never
/// propagated to the caller, because a missing MQTT connection must not
/// prevent the serial port from operating.
#[derive(Clone)]
pub(crate) struct MqttHandle {
    client: Arc<AsyncClient>,
}

impl MqttHandle {
    /// Connects to the broker described by `config` and spawns the background
    /// event-loop task that keeps the connection alive.
    pub async fn connect(config: &MqttConfig) -> Result<Self, crate::Error> {
        let client_id = format!(
            "{}-{}",
            config.client_id_prefix,
            generate_short_id()
        );

        let mut opts =
            MqttOptions::new(&client_id, &config.broker_host, config.broker_port);
        opts.set_keep_alive(Duration::from_secs(config.keep_alive_secs));

        let (client, event_loop) = AsyncClient::new(opts, 100);

        // Spawn the event-loop driver — it reconnects automatically on error.
        tokio::spawn(mqtt_event_loop(event_loop));

        Ok(Self {
            client: Arc::new(client),
        })
    }

    /// Publishes `payload` to the lulu-logs topic
    /// `lulu/{source}/{attribute}` with QoS 0 (at most once).
    ///
    /// Errors are silently logged — see the type-level documentation.
    pub async fn publish(&self, source: &str, attribute: &str, payload: &str) {
        let topic = format!("lulu/{}/{}", source, attribute);
        if let Err(e) = self
            .client
            .publish(&topic, QoS::AtMostOnce, false, payload.as_bytes().to_vec())
            .await
        {
            tracing::warn!("mqtt publish to {}: {}", topic, e);
        }
    }
}

// ---------------------------------------------------------------------------
// Background event-loop
// ---------------------------------------------------------------------------

/// Drives the `rumqttc` event loop with exponential back-off on errors.
async fn mqtt_event_loop(mut event_loop: EventLoop) {
    let mut backoff_ms: u64 = 1_000;
    const MAX_BACKOFF_MS: u64 = 30_000;

    loop {
        match event_loop.poll().await {
            Ok(_notification) => {
                backoff_ms = 1_000; // reset on any successful event
            }
            Err(e) => {
                tracing::warn!("mqtt connection error: {} — retrying in {} ms", e, backoff_ms);
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generates a short random alphanumeric string for use in MQTT client IDs.
fn generate_short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{:08x}", nanos)
}
