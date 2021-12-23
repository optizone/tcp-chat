mod event;

use chat::client::Client;
use chat::MessageType;
use event::*;
use regex::Regex;
use std::path::PathBuf;
use std::{error::Error, io};
use std::{net::SocketAddr, str::FromStr};
use structopt::StructOpt;
use termion::{event::Key, input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};

#[derive(Debug, StructOpt)]
#[structopt(name = "client", about = "Simple TCP chat room.")]
struct Opt {
    /// Set address of the server
    #[structopt(short, long, default_value = "127.0.0.1:8080")]
    address: String,

    /// Username can contain only english characters, numbers and underscores and must be encoded with UTF-8 and less or equal to 32 characters
    #[structopt(short, long)]
    username: String,

    #[structopt(short, long, default_value = ".")]
    save_directory: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // console_subscriber::init();
    let Opt {
        address,
        username,
        save_directory,
    } = Opt::from_args();
    let addr = SocketAddr::from_str(address.as_str()).unwrap();
    let title_text = format!("{} as {}", address, username);
    let client = Client::new(username.clone(), addr, save_directory)
        .await
        .unwrap();

    // Terminal initialization
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut events = Events::new(client);
    let mut messages = vec![];
    let mut curr_text = String::new();

    let mut offset = 0u16;

    loop {
        let p_m = messages.iter().cloned().collect::<Vec<_>>();
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Percentage(10),
                        Constraint::Percentage(80),
                        Constraint::Percentage(10),
                    ]
                    .as_ref(),
                )
                .split(f.size());

            let title_area = Paragraph::new(Span::raw(&title_text))
                .block(Block::default().title("Chat Room").borders(Borders::ALL))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true });
            let messages_area = Paragraph::new(p_m)
                .block(Block::default().title("Paragraph").borders(Borders::ALL))
                .alignment(Alignment::Left)
                .scroll((offset, 0))
                .wrap(Wrap { trim: true });
            let type_area = Paragraph::new(Span::raw(curr_text.as_str()))
                .block(
                    Block::default()
                        .title("Type your message here")
                        .borders(Borders::ALL),
                )
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true });
            f.render_widget(title_area, chunks[0]);
            f.render_widget(messages_area, chunks[1]);
            f.render_widget(type_area, chunks[2]);
        })?;

        match events.next()? {
            Event::Input(Key::Char('\n')) => {
                lazy_static::lazy_static! {
                    static ref RE: Regex = Regex::new(r"((/file (?P<file>((?:[a-zA-Z]|\\)(\\[\w\- \.:]+\.(\w+))|((/[\w\- \.:]+)+)))$)|(?P<msg>.*))").unwrap();
                }
                let (file, message) = {
                    let c = RE.captures(&curr_text).unwrap();
                    (
                        c.name("file").map(|m| PathBuf::from(m.as_str())),
                        c.name("msg").map(|m| m.as_str().to_string()),
                    )
                };
                if let Some(file) = file {
                    events.send_file(file).await;
                } else {
                    events.send(message.unwrap()).await;
                }
                curr_text.clear();
            }
            Event::Input(Key::Backspace) => {
                curr_text.pop();
            }
            Event::Input(Key::Char(ch)) => {
                curr_text.push(ch);
            }
            Event::Input(Key::Down) => {
                offset += 1;
            }
            Event::Input(Key::Up) => {
                offset = offset.saturating_sub(1);
            }
            Event::Input(Key::Esc) => {
                break;
            }
            Event::Recv(msg) => {
                let time = msg
                    .timestamp
                    .naive_local()
                    .time()
                    .format("%H:%M:%S")
                    .to_string();
                let user: String = msg.from.into();
                let user_color = if user == username {
                    Color::Yellow
                } else {
                    Color::Blue
                };
                match msg.desc.r#type {
                    MessageType::File => {
                        messages.push(Spans::from(vec![
                            Span::styled(
                                format!("<{}> ", time),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("[{}] send file: ", user),
                                Style::default().fg(user_color),
                            ),
                            Span::styled(
                                msg.filename.unwrap(),
                                Style::default().add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                    }
                    MessageType::Utf8 => {
                        messages.push(Spans::from(vec![
                            Span::styled(
                                format!("<{}> ", time),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!("[{}]: ", user), Style::default().fg(user_color)),
                            Span::raw(String::from_utf8(msg.content).unwrap()),
                        ]));
                    }
                    MessageType::Login => {
                        messages.push(Spans::from(vec![
                            Span::styled(
                                format!("<{}> ", time),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("Welcome our new user! "),
                            Span::styled(user, Style::default().fg(Color::Red)),
                        ]));
                    }
                    MessageType::Logout => {
                        messages.push(Spans::from(vec![
                            Span::styled(
                                format!("<{}> ", time),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(user, Style::default().fg(Color::Red)),
                            Span::raw(" left the chat."),
                        ]));
                    }
                    _ => continue,
                }
            }
            _ => {}
        }
    }

    Ok(())
}
