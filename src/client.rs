use std::{path::PathBuf, pin::Pin};

use chrono::{DateTime, Utc};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        mpsc::{channel, Receiver, Sender},
        Mutex,
    },
};

use crate::{Descriptor, MessageType, ServerHeader};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("IO: {0}")]
    Io(#[from] tokio::io::Error),

    #[error("Bad username")]
    BadUsername,
}

#[derive(Debug)]
pub struct ServerMessage {
    pub desc: Descriptor,
    pub timestamp: DateTime<Utc>,
    pub from: String,
    pub filename: Option<String>,
    pub content: Vec<u8>,
}

pub struct Client {
    reciever: Mutex<Receiver<ServerMessage>>,
    sender: Mutex<Sender<ClientMessage>>,
    save_dir: PathBuf,
}

impl Client {
    pub async fn new(
        uname: String,
        addr: impl ToSocketAddrs,
        save_dir: PathBuf,
    ) -> Result<Self, Error> {
        let (reader, writer) = TcpStream::connect(addr).await?.into_split();
        let mut reader = BufReader::new(reader);
        let mut writer = BufWriter::new(writer);
        let (tx_c, mut rx_c) = channel(128);
        let (tx_s, rx_s) = channel(128);

        writer
            .write_all(
                Descriptor::from(MessageType::Login)
                    .with_header_len(uname.len() as u16)
                    .as_bytes(),
            )
            .await?;
        writer.write_all(uname.as_bytes()).await?;
        writer.flush().await?;

        let desc = Descriptor::read(Pin::new(&mut reader)).await?;
        if desc.r#type != MessageType::Login {
            return Err(Error::BadUsername);
        }

        tokio::spawn(async move {
            while let Some(msg) = rx_c.recv().await {
                match msg {
                    ClientMessage::File(path) => {
                        let file = File::open(&path).await.unwrap();
                        let filename = path.file_name().unwrap().to_string_lossy();
                        writer
                            .write_all(
                                Descriptor::from(MessageType::File)
                                    .with_header_len(filename.len() as u16)
                                    .with_content_len(file.metadata().await.unwrap().len() as u64)
                                    .as_bytes(),
                            )
                            .await
                            .unwrap();
                        writer.write_all(filename.as_bytes()).await.unwrap();
                        let mut reader = BufReader::new(file);
                        let mut buf = Vec::with_capacity(1024);
                        while reader.read_buf(&mut buf).await.unwrap() != 0 {
                            writer.write_all(&buf).await.unwrap();
                            buf.clear();
                        }
                        writer.flush().await.unwrap();
                    }
                    ClientMessage::Utf8(text) => {
                        writer
                            .write_all(
                                Descriptor::from(MessageType::Utf8)
                                    .with_content_len(text.len() as u64)
                                    .as_bytes(),
                            )
                            .await
                            .unwrap();
                        writer.write_all(text.as_bytes()).await.unwrap();
                        writer.flush().await.unwrap();
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut buf = Vec::new();
            loop {
                let (desc, header, content) =
                    read_msg(Pin::new(&mut reader), &mut buf).await.unwrap();
                let msg = ServerMessage {
                    desc,
                    timestamp: header.timestamp,
                    from: header.from.into(),
                    filename: header.filename.map(|v| v.into()),
                    content,
                };
                tx_s.send(msg).await.unwrap();
            }
        });

        Ok(Self {
            reciever: Mutex::new(rx_s),
            sender: Mutex::new(tx_c),
            save_dir,
        })
    }

    pub async fn recv(&self) -> ServerMessage {
        let msg = self.reciever.lock().await.recv().await.unwrap();
        if msg.desc.r#type == MessageType::File {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .open(self.save_dir.join(msg.filename.as_ref().unwrap()))
                .await
                .unwrap();
            file.write_all(&msg.content).await.unwrap();
        }
        msg
    }

    pub async fn send_text(&self, text: String) {
        self.sender
            .lock()
            .await
            .send(ClientMessage::Utf8(text))
            .await
            .unwrap()
    }

    pub async fn send_file(&self, path: PathBuf) {
        self.sender
            .lock()
            .await
            .send(ClientMessage::File(path))
            .await
            .unwrap()
    }
}

async fn read_msg<'h, R: AsyncReadExt>(
    mut reader: Pin<&mut R>,
    header_buf: &'h mut Vec<u8>,
) -> Result<(Descriptor, ServerHeader<'h, 'h>, Vec<u8>), Error> {
    let desc = Descriptor::read(Pin::new(&mut reader)).await?;
    let mut content = Vec::new();
    header_buf.resize(desc.header_len as usize, 0u8);
    content.resize(desc.content_len as usize, 0u8);
    reader.read_exact(header_buf).await?;
    reader.read_exact(&mut content).await?;
    Ok((desc, serde_json::from_slice(header_buf).unwrap(), content))
}

#[derive(Debug)]
enum ClientMessage {
    Utf8(String),
    File(PathBuf),
}
