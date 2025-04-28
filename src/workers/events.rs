use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub struct BranchEvent {
    pub repo: String,
    pub branch: String,
}

#[derive(Clone, Debug)]
pub struct Hub {
    sender: broadcast::Sender<BranchEvent>,
}

impl Hub {
    pub fn new(buffer: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BranchEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: BranchEvent) {
        let _ = self.sender.send(event);
    }
}
