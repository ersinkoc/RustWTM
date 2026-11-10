//! One bounded, detached operation per view. Dropped views never queue work.
use eframe::egui;
use std::sync::mpsc::{self, Receiver};
pub struct Task<T> {
    receiver: Option<Receiver<Result<T, String>>>,
}
impl<T> Default for Task<T> {
    fn default() -> Self {
        Self { receiver: None }
    }
}
impl<T: Send + 'static> Task<T> {
    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn start(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.receiver = Some(rx);
        let ctx = ctx.clone();
        #[cfg(not(test))]
        if let Err(e) = std::thread::Builder::new()
            .name("product-query".into())
            .spawn(move || {
                let _ = tx.send(work());
                ctx.request_repaint();
            })
        {
            self.receiver = None;
            let (tx, rx) = mpsc::sync_channel(1);
            let _ = tx.send(Err(e.to_string()));
            self.receiver = Some(rx);
        }
        #[cfg(test)]
        {
            let _ = (&ctx, &work);
            let _ = tx.send(Err("Synthetic view: use a fixture".into()));
        }
    }
    pub fn poll(&mut self) -> Option<Result<T, String>> {
        let rx = self.receiver.as_ref()?;
        match rx.try_recv() {
            Ok(result) => {
                self.receiver = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                Some(Err("Background operation stopped".into()))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
