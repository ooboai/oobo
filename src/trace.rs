use std::sync::atomic::{AtomicU64, Ordering};

static TRACE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Legacy manual trace helper. Still used in `interceptor.rs` for span-style
/// timing alongside the new `tracing` crate instrumentation.
#[derive(Debug, Clone)]
pub struct Trace {
    id: Option<String>,
}

impl Trace {
    pub fn new(operation: &str) -> Self {
        if !enabled() {
            return Self { id: None };
        }

        let n = TRACE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!(
            "{}-{}-{n}",
            std::process::id(),
            chrono::Utc::now().timestamp()
        );
        tracing::debug!(trace_id = %id, operation, "trace start");
        Self { id: Some(id) }
    }

    pub fn stage(&self, stage: &str) {
        if let Some(id) = &self.id {
            tracing::debug!(trace_id = %id, stage, "trace stage");
        }
    }

    pub fn detail(&self, stage: &str, detail: impl AsRef<str>) {
        if let Some(id) = &self.id {
            tracing::debug!(trace_id = %id, stage, detail = detail.as_ref(), "trace detail");
        }
    }
}

fn enabled() -> bool {
    std::env::var("OOBO_TRACE").is_ok_and(|v| trace_value_enabled(&v))
}

fn trace_value_enabled(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    !(v.is_empty() || v == "0" || v == "false" || v == "off")
}

// ── Structured tracing initialization ─────────────────────────────────

/// Initialize the `tracing` subscriber based on `OOBO_DEBUG` env var.
///
/// - Default (unset or `0`): no subscriber installed — zero cost.
/// - `OOBO_DEBUG=1`: compact file appender to `<oobo_home>/logs/anchor.log`.
/// - `OOBO_DEBUG=2`: file appender + stderr output.
///
/// `OOBO_LOG` can fine-tune filtering (e.g. `OOBO_LOG=anchor::git=debug`).
/// Default filter when debug is on: `info` for the `anchor` crate, `warn`
/// for everything else.
pub fn init() {
    let debug_level = match std::env::var("OOBO_DEBUG") {
        Ok(v) => match v.trim() {
            "1" => 1,
            "2" => 2,
            v if v.parse::<u32>().unwrap_or(0) >= 2 => 2,
            v if !v.is_empty() && v != "0" && v != "false" && v != "off" => 1,
            _ => return,
        },
        Err(_) => return,
    };

    let default_filter = "anchor=info,oobo=info,warn";
    let filter = std::env::var("OOBO_LOG").unwrap_or_else(|_| default_filter.to_string());

    let env_filter = tracing_subscriber::EnvFilter::builder().parse_lossy(&filter);

    let log_dir = log_directory();
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::daily(&log_dir, "oobo.log");

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let file_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_ansi(false)
        .with_writer(file_appender);

    if debug_level >= 2 {
        let stderr_layer = tracing_subscriber::fmt::layer()
            .compact()
            .with_writer(std::io::stderr);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(file_layer)
            .with(stderr_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(file_layer)
            .init();
    }
}

/// Resolve the log directory, preferring XDG but falling back to legacy.
fn log_directory() -> std::path::PathBuf {
    crate::paths::oobo_home().join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_false_values_disable() {
        for value in ["", "0", "false", "off"] {
            assert!(
                !trace_value_enabled(value),
                "{value:?} should disable tracing"
            );
        }
    }

    #[test]
    fn trace_enabled_for_truthy_values() {
        for value in ["1", "true", "debug", "yes"] {
            assert!(
                trace_value_enabled(value),
                "{value:?} should enable tracing"
            );
        }
    }
}
