use chrono::{DateTime, Utc};

pub fn utc_now() -> DateTime<Utc> {
    utc_now_impl()
}

pub struct Stopwatch {
    inner: StopwatchInner,
}

impl Stopwatch {
    pub fn start_new() -> Self {
        Self {
            inner: StopwatchInner::start_new(),
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.inner.elapsed_ms()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn utc_now_impl() -> DateTime<Utc> {
    Utc::now()
}

#[cfg(target_arch = "wasm32")]
fn utc_now_impl() -> DateTime<Utc> {
    let millis = js_sys::Date::new_0().get_time() as i64;
    DateTime::from_timestamp_millis(millis)
        .expect("JavaScript Date should always convert to a UTC timestamp")
}

#[cfg(not(target_arch = "wasm32"))]
struct StopwatchInner(std::time::Instant);

#[cfg(not(target_arch = "wasm32"))]
impl StopwatchInner {
    fn start_new() -> Self {
        Self(std::time::Instant::now())
    }

    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}

#[cfg(target_arch = "wasm32")]
struct StopwatchInner(f64);

#[cfg(target_arch = "wasm32")]
impl StopwatchInner {
    fn start_new() -> Self {
        Self(js_sys::Date::new_0().get_time())
    }

    fn elapsed_ms(&self) -> u64 {
        (js_sys::Date::new_0().get_time() - self.0).max(0.0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::{Stopwatch, utc_now};

    #[test]
    fn utc_now_returns_a_reasonable_timestamp() {
        let now = utc_now();
        assert!(now.timestamp() > 1_700_000_000);
    }

    #[test]
    fn stopwatch_reports_elapsed_time() {
        let watch = Stopwatch::start_new();
        assert!(watch.elapsed_ms() < 1_000);
    }
}
