use parking_lot::RwLock;
use std::collections::VecDeque;

#[derive(Debug)]
pub struct MessageQueue {
    q: RwLock<VecDeque<(String, String)>>,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self {
            q: RwLock::new(VecDeque::new()),
        }
    }

    pub fn add_message(&self, m: String) {
        let now = chrono::offset::Utc::now();
        let formatted = now.format("%v %T %Z");

        let mut qq = self.q.write();
        qq.push_front((formatted.to_string(), m));
        qq.truncate(25);
    }

    pub fn get_messages(&self) -> Vec<(String, String)> {
        let mut cloned = {
            let qq = self.q.read();
            qq.clone()
        };

        cloned.drain(..).collect()
    }
}
