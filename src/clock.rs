use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Monotonic seconds, comparable across threads. Their zero is the first call.
pub fn now() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}
