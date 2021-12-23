use std::io;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread;

use termion::event::Key;
use termion::input::TermRead;

use chat::client::*;

// #[allow(dead_code)]

pub enum Event {
    Input(Key),
    Recv(ServerMessage),
    Tick,
}

pub struct Events {
    rx: mpsc::Receiver<Event>,
    client: Arc<Client>,
}

impl Events {
    pub fn new(client: Client) -> Events {
        let client = Arc::new(client);
        let recv_client = Arc::clone(&client);

        let (tx, rx) = mpsc::channel();
        {
            let tx = tx.clone();
            thread::spawn(move || {
                let stdin = io::stdin();
                for evt in stdin.keys() {
                    if let Ok(key) = evt {
                        if let Err(err) = tx.send(Event::Input(key)) {
                            eprintln!("{}", err);
                            return;
                        }
                    }
                }
            })
        };

        {
            let tx = tx.clone();
            thread::spawn(move || loop {
                thread::sleep(std::time::Duration::from_millis(100));
                if let Err(err) = tx.send(Event::Tick) {
                    eprintln!("{}", err);
                    return;
                }
            })
        };

        {
            let tx = tx.clone();
            tokio::spawn(async move {
                loop {
                    let msg = recv_client.recv().await;
                    if let Err(err) = tx.send(Event::Recv(msg)) {
                        eprintln!("{}", err);
                        return;
                    }
                }
            })
        };

        Events { rx, client }
    }

    pub fn next(&self) -> Result<Event, mpsc::RecvError> {
        self.rx.recv()
    }

    pub async fn send(&mut self, message: String) {
        self.client.send_text(message).await;
    }

    pub async fn send_file(&mut self, file: PathBuf) {
        self.client.send_file(file).await;
    }
}
