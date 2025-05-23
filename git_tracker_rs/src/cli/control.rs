use crate::cli::messages::ControlCommand;
use crate::supervisor::Supervisor;

use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

pub async fn start_control_interface(supervisor: Arc<Mutex<Supervisor>>) {
    let listener = TcpListener::bind("127.0.0.1:4000").await.unwrap();

    loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        let supervisor = supervisor.clone();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            let n = match socket.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!("Failed to read from socket: {}", e);
                    return;
                }
            };

            if let Ok(cmd) = serde_json::from_slice::<ControlCommand>(&buf[..n]) {
                println!("Parsed control command: {:?}", cmd);

                let mut sup = supervisor.lock().await;
                match cmd {
                    ControlCommand::CheckNow(name) => {
                        sup.send_check_now(&name).await;
                    }
                    ControlCommand::Shutdown(name) => {
                        sup.send_shutdown(&name).await;
                    }
                    ControlCommand::UpdateConfigPartial(config) => {
                        sup.update_config(config).await;
                    }
                    ControlCommand::ShutdownAll => {
                        sup.shutdown_all().await;
                    }
                }
            }
        });
    }
}

