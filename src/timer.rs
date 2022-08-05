use tokio::time::{self, sleep, Duration, Instant, Interval};

pub enum TickResult {
    Completed,
    Paused,
}

pub struct PausableInterval {
    delay: Interval,

    is_paused: bool,

    already_expired: Option<Duration>,
    last_interaction: Instant,
}

impl PausableInterval {
    pub fn new(interval: Duration) -> Self {
        let mut delay = time::interval(interval);
        delay.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        Self {
            delay,
            is_paused: false,
            already_expired: None,
            last_interaction: Instant::now(),
        }
    }

    pub async fn tick(&mut self) -> TickResult {
        if self.is_paused() {
            return TickResult::Paused;
        }

        if let Some(mut expired) = self.already_expired.take() {
            expired += Instant::now() - self.last_interaction;

            let duration = self.delay.period().saturating_sub(expired);
            sleep(duration).await;
            self.delay.reset();
        } else {
            self.delay.tick().await;
        }

        self.last_interaction = Instant::now();

        TickResult::Completed
    }

    /// Return whether this intervall is currently paused or not.
    pub fn is_paused(&self) -> bool {
        self.is_paused
    }

    /// Pause this intervall, preventing pending await on `tick()` to return.
    /// Pausing an already paused intervall is a no-op.
    pub fn pause(&mut self, paused: bool) {
        if self.is_paused() == paused {
            return;
        }

        self.is_paused = paused;

        if paused {
            self.already_expired = Some(
                self.already_expired.unwrap_or_default() + (Instant::now() - self.last_interaction),
            );
        }

        self.last_interaction = Instant::now();
    }

    pub fn reset(&mut self) {
        self.already_expired = None;
        self.last_interaction = Instant::now();
        self.delay.reset()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use tokio::time::sleep;

    async fn measure<T>(fun: impl std::future::Future<Output = T>) -> Duration {
        let begin = Instant::now();
        fun.await;
        let end = Instant::now();
        end - begin
    }

    #[tokio::test(start_paused = true)]
    async fn test_pause_schedules_remaining() {
        let mut interval = PausableInterval::new(Duration::from_secs(1));

        sleep(Duration::from_secs_f32(0.5)).await;

        interval.pause(true);

        let begin = Instant::now();
        assert!(matches!(interval.tick().await, TickResult::Paused));
        let end = Instant::now();

        assert_eq!(end - begin, Duration::new(0, 0));

        sleep(Duration::from_secs_f32(17.345)).await;

        interval.pause(false);

        let begin = Instant::now();

        assert!(matches!(interval.tick().await, TickResult::Completed));

        let end = Instant::now();

        assert_eq!(end - begin, Duration::from_secs_f32(0.5));
    }

    #[tokio::test(start_paused = true)]
    async fn test_unpaused_time_accumulates() {
        let mut interval = PausableInterval::new(Duration::from_secs(100));

        sleep(Duration::from_secs(20)).await; // total 20

        interval.pause(true);

        // This sleep should not count
        sleep(Duration::from_secs(20)).await;

        interval.pause(false);

        sleep(Duration::from_secs(20)).await; // total 40

        interval.pause(true);

        // neither should this one
        sleep(Duration::from_secs(20)).await;

        interval.pause(false);

        sleep(Duration::from_secs(20)).await; // total 60

        assert_eq!(
            measure(interval.tick()).await,
            Duration::from_secs(100 - 60)
        );
    }
}
