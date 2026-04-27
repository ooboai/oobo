use std::sync::atomic::{AtomicU64, Ordering};

static TRACE_COUNTER: AtomicU64 = AtomicU64::new(1);

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
        eprintln!("anchor: trace {id} start {operation}");
        Self { id: Some(id) }
    }

    pub fn stage(&self, stage: &str) {
        if let Some(id) = &self.id {
            eprintln!("anchor: trace {id} {stage}");
        }
    }

    pub fn detail(&self, stage: &str, detail: impl AsRef<str>) {
        if let Some(id) = &self.id {
            eprintln!("anchor: trace {id} {stage}: {}", detail.as_ref());
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
