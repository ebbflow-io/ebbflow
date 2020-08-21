use crate::config::ConcreteHealthCheck;
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc};

pub struct HealthData {}

pub struct HealthMaster {
    inner: Arc<Mutex<HealthMasterInner>>,
    pub cfg: ConcreteHealthCheck,
}

impl HealthMaster {
    pub fn new(cfg: ConcreteHealthCheck) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HealthMasterInner::new())),
            cfg,
        }
    }

    pub fn report_check_result(&self, result: bool) -> bool {
        self.inner.lock().report_check_result(result, &self.cfg)
    }

    pub fn data(&self) -> (bool, VecDeque<(bool, u128)>) {
        self.inner.lock().data()
    }
}

pub struct HealthMasterInner {
    healthy: bool,
    recent: VecDeque<(bool, u128)>,
}

impl HealthMasterInner {
    pub fn new() -> Self {
        Self {
            healthy: false,
            recent: VecDeque::new(),
        }
    }

    pub fn report_check_result(&mut self, result: bool, cfg: &ConcreteHealthCheck) -> bool {
        use std::time::SystemTime;

        let t = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(n) => n.as_millis(),
            Err(_) => 0,
        };
        self.recent.push_front((result, t));
        self.status(cfg)
    }

    pub fn data(&self) -> (bool, VecDeque<(bool, u128)>) {
        (self.healthy, self.recent.clone())
    }

    pub fn status(&mut self, cfg: &ConcreteHealthCheck) -> bool {
        while self.recent.len()
            > std::cmp::max(
                cfg.consider_healthy_threshold as usize,
                cfg.consider_unhealthy_threshold as usize,
            )
        {
            self.recent.pop_back();
        }

        if self.healthy {
            // we need X consecutive unhealthies
            let mut unhealthies = 0;
            for (check, _time) in self.recent.iter() {
                if *check {
                    break;
                } else {
                    unhealthies += 1;
                }
            }
            // We were healthy, but had `unhealthies` consecutive unhealthies, so we are now unhealthy
            if unhealthies >= cfg.consider_unhealthy_threshold {
                self.healthy = false;
                self.healthy
            // We are healthy, but did not make the threshold, so we are healthy
            } else {
                self.healthy = true;
                self.healthy
            }
        } else {
            // we need X consecutive healthies
            let mut healthies = 0;
            for (check, _time) in self.recent.iter() {
                if *check {
                    healthies += 1;
                } else {
                    break;
                }
            }
            // We were unhealthy, but had `healthies` consecutive healthies, so we are now healthy
            if healthies >= cfg.consider_healthy_threshold {
                self.healthy = true;
                self.healthy
            // We are unhealthy, but did not make the threshold, so we are unhealthy
            } else {
                self.healthy = false;
                self.healthy
            }
        }
    }
}
