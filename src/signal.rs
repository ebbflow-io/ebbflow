use std::sync::Arc;
use tokio::sync::watch::{channel, Receiver, Sender};

#[derive(Clone)]
pub struct SignalSender {
    sender: Arc<Sender<bool>>,
    receiver: Receiver<bool>,
}

impl SignalSender {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (s, r) = channel(false);
        Self {
            sender: Arc::new(s),
            receiver: r,
        }
    }

    pub fn new_receiver(&self) -> SignalReceiver {
        SignalReceiver::new(self.receiver.clone())
    }

    pub fn send_signal(&self) {
        let _ = self.sender.broadcast(true);
    }
}

#[derive(Clone)]
pub struct SignalReceiver {
    receiver: Receiver<bool>,
}

impl SignalReceiver {
    fn new(receiver: Receiver<bool>) -> Self {
        Self { receiver }
    }

    pub async fn wait(&mut self) {
        loop {
            let r = self.receiver.recv().await;
            match r {
                None => break,
                Some(b) => {
                    if b {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod signaltests {
    use super::SignalSender;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::delay_for;

    #[tokio::test]
    async fn basic() {
        let s = SignalSender::new();
        let mut r = s.new_receiver();

        let flipper = Arc::new(AtomicBool::new(false));
        let flipperc = flipper.clone();

        let _ = tokio::spawn(async move {
            r.wait().await;
            flipperc.store(true, Ordering::SeqCst);
        });

        delay_for(Duration::from_millis(50)).await;
        assert!(!flipper.load(Ordering::SeqCst));
        s.send_signal();
        delay_for(Duration::from_millis(50)).await;
        assert!(flipper.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn basic_cloned_receivers() {
        let s = SignalSender::new();
        let r = s.new_receiver();

        let n = 4;

        let flipper = Arc::new(AtomicUsize::new(0));

        for _i in 0..n {
            let flipperc = flipper.clone();
            let mut rc = r.clone();
            tokio::spawn(async move {
                rc.wait().await;
                flipperc.fetch_add(1, Ordering::SeqCst);
            });
        }

        delay_for(Duration::from_millis(50)).await;
        assert_eq!(0, flipper.load(Ordering::SeqCst));
        s.send_signal();
        delay_for(Duration::from_millis(50)).await;
        assert_eq!(n, flipper.load(Ordering::SeqCst));
    }
}
