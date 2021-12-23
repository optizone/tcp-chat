use std::{collections::HashMap, env::temp_dir, path::PathBuf, pin::Pin, sync::Arc};

use chrono::Utc;
use tokio::{
    fs::{File, OpenOptions},
    io::{self, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream, ToSocketAddrs,
    },
    sync::{
        mpsc::{channel, Receiver, Sender},
        oneshot,
    },
};

const BUF_SIZE: usize = 16 * 1024;

use crate::{Descriptor, MessageType, ServerHeader};

#[derive(Debug, Clone)]
enum Content {
    Vec(Arc<Vec<u8>>),
    File(Arc<PathBuf>),
    None,
}

#[derive(Debug)]
enum InternalMessage {
    Message {
        desc: Descriptor,
        header: Arc<Vec<u8>>,
        content: Content,
    },
    Join {
        username: Arc<String>,
        resp: oneshot::Sender<MessageType>,
        sender: Sender<InternalMessage>,
    },
    Logout {
        username: Arc<String>,
    },
}

impl InternalMessage {
    fn try_clone(&self) -> Option<Self> {
        match &self {
            Self::Message {
                desc,
                header,
                content,
            } => Some(Self::Message {
                desc: *desc,
                header: Arc::clone(header),
                content: content.clone(),
            }),
            _ => None,
        }
    }
}

pub async fn run_server(addrs: impl ToSocketAddrs) -> io::Result<()> {
    let listener = TcpListener::bind(addrs).await?;
    let (tx, rx) = channel(128);

    let tx_c = tx.clone();
    tokio::spawn(async move { server_task(rx, tx_c).await });

    loop {
        let (stream, _) = listener.accept().await?;
        let tx = tx.clone();
        tokio::spawn(async move { handle_connection(stream, tx).await });
    }
}

async fn server_task(
    mut rx: Receiver<InternalMessage>,
    tx: Sender<InternalMessage>,
) -> io::Result<()> {
    let mut map = HashMap::<Arc<String>, Sender<InternalMessage>>::new();
    while let Some(msg) = rx.recv().await {
        match msg {
            msg @ InternalMessage::Message { .. } => {
                for (_, sender) in map.iter_mut() {
                    let _ = sender.send(msg.try_clone().unwrap()).await;
                }
            }
            InternalMessage::Join {
                username,
                resp,
                sender,
            } => {
                let header = Arc::new(
                    ServerHeader::default()
                        .with_username(username.as_str())
                        .to_json(),
                );
                if let Some(_) = map.insert(username, sender) {
                    let _ = resp.send(MessageType::UsernameExists);
                } else {
                    let _ = tx
                        .send(InternalMessage::Message {
                            desc: Descriptor::from(MessageType::Login)
                                .with_header_len(header.len() as u16),
                            header,
                            content: Content::None,
                        })
                        .await
                        .unwrap();
                    let _ = resp.send(MessageType::Login);
                }
            }
            InternalMessage::Logout { username } => {
                map.remove(&username);
                let header = Arc::new(
                    ServerHeader::default()
                        .with_username(username.as_str())
                        .to_json(),
                );
                let _ = tx
                    .send(InternalMessage::Message {
                        desc: Descriptor::from(MessageType::Logout)
                            .with_header_len(header.len() as u16),
                        header,
                        content: Content::None,
                    })
                    .await
                    .unwrap();
            }
        }
    }

    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    mut sender: Sender<InternalMessage>,
) -> io::Result<()> {
    let (reader, writer) = stream.into_split();

    let mut reader = BufReader::new(reader);
    let mut writer = BufWriter::new(writer);

    let (tx, mut rx) = channel(128);

    let username = process_login(&mut reader, &mut writer, &mut sender, tx).await?;
    let uname = username.as_str();

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                InternalMessage::Message {
                    desc,
                    header,
                    content,
                } => {
                    writer.write_all(desc.as_bytes()).await?;
                    writer.write_all(header.as_slice()).await?;
                    content.write(Pin::new(&mut writer)).await?;
                    writer.flush().await?;
                }
                _ => unreachable!(),
            }
        }
        io::Result::Ok(())
    });

    while let Ok(_) = process_msg(uname, &mut reader, &mut sender).await {}

    sender
        .send(InternalMessage::Logout { username })
        .await
        .unwrap();

    Ok(())
}

async fn process_login(
    reader: &mut BufReader<OwnedReadHalf>,
    writer: &mut BufWriter<OwnedWriteHalf>,
    sender: &mut Sender<InternalMessage>,
    sender_conn: Sender<InternalMessage>,
) -> io::Result<Arc<String>> {
    loop {
        let desc = Descriptor::read(Pin::new(reader)).await?;
        if desc.r#type != MessageType::Login {
            let desc = Descriptor::from(MessageType::BadLogin);
            send_msg(writer, desc, None, None).await?;
            continue;
        }
        let mut username = Vec::with_capacity(desc.header_len as usize);
        unsafe {
            username.set_len(desc.header_len as usize);
        }
        reader.read_exact(&mut username).await?;
        let username = match String::from_utf8(username) {
            Ok(u) => Arc::new(u),
            Err(_) => {
                let desc = Descriptor::from(MessageType::BadUsername);
                send_msg(writer, desc, None, None).await?;
                continue;
            }
        };

        let (resp, recv) = oneshot::channel();
        sender
            .send(InternalMessage::Join {
                username: Arc::clone(&username),
                resp,
                sender: sender_conn.clone(),
            })
            .await
            .unwrap();
        let resp = recv.await.expect("sender should not be dropped!");
        let desc = Descriptor::from(resp);
        send_msg(writer, desc, None, None).await?;
        if resp == MessageType::Login {
            break Ok(username);
        }
    }
}

async fn process_msg(
    uname: &str,
    reader: &mut BufReader<OwnedReadHalf>,
    sender: &mut Sender<InternalMessage>,
) -> io::Result<()> {
    let desc = Descriptor::read(Pin::new(reader)).await?;
    match desc.r#type {
        MessageType::Utf8 | MessageType::File | MessageType::Voice | MessageType::Image => {}
        _ => todo!(),
    }
    let filename = if desc.r#type == MessageType::File {
        // TODO make it use object pool
        let mut buf = Vec::new();
        buf.resize(desc.header_len as usize, 0);
        reader.read_exact(&mut buf).await?;
        Some(String::from_utf8(buf).unwrap_or_default())
    } else {
        None
    };

    let content = if desc.content_len <= BUF_SIZE as u64 {
        // TODO make it use object pool
        let mut buf = Vec::new();
        buf.resize(desc.content_len as usize, 0);
        reader.read_exact(&mut buf).await?;
        Content::Vec(Arc::new(buf))
    } else {
        let path = Arc::new(temp_dir().join(uuid::Uuid::new_v4().to_string()));
        let mut writer = BufWriter::new(OpenOptions::new().create(true).open(path.as_ref()).await?);
        let mut buf = Vec::with_capacity(BUF_SIZE);
        while reader.read_buf(&mut buf).await? != 0 {
            writer.write_all(&buf).await?;
            buf.clear();
        }
        Content::File(path)
    };

    let header = ServerHeader {
        timestamp: Utc::now(),
        from: uname,
        filename: filename.as_ref().map(|p| p.as_str()),
    };

    let header = Arc::new(serde_json::to_vec(&header).unwrap());
    sender
        .send(InternalMessage::Message {
            desc: desc.with_header_len(header.len() as u16),
            header,
            content,
        })
        .await
        .unwrap();

    Ok(())
}

async fn send_msg(
    writer: &mut BufWriter<OwnedWriteHalf>,
    desc: Descriptor,
    header: Option<Arc<Vec<u8>>>,
    content: Option<Arc<Vec<u8>>>,
) -> io::Result<()> {
    writer.write_all(desc.as_bytes()).await?;
    if let Some(header) = header {
        writer.write_all(header.as_slice()).await?;
    }
    if let Some(content) = content {
        writer.write_all(content.as_slice()).await?;
    }
    writer.flush().await?;
    Ok(())
}

impl Content {
    async fn write<W: AsyncWriteExt>(self, mut writer: Pin<&mut W>) -> io::Result<()> {
        match self {
            Content::Vec(v) => {
                writer.write_all(v.as_slice()).await?;
            }
            Content::File(path) => {
                // TODO make it use object pool
                let mut buf = Vec::with_capacity(BUF_SIZE);
                let mut reader = BufReader::new(File::open(path.as_ref()).await?);
                while reader.read_buf(&mut buf).await? != 0 {
                    writer.write_all(&buf).await?;
                    buf.clear();
                }
            }
            Content::None => {}
        }
        Ok(())
    }
}
