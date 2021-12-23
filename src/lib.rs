use std::pin::Pin;

use chrono::{DateTime, Utc};
use num_enum::FromPrimitive;
use serde::{Deserialize, Serialize};
use tokio::io::{self, AsyncReadExt};

pub mod client;
pub mod server;

#[repr(u16)]
#[derive(FromPrimitive, PartialEq, Eq, Clone, Copy, Debug)]
pub enum MessageType {
    Login = 1,
    Logout = 2,

    UsernameExists = 3,
    BadUsername = 4,
    BadLogin = 5,

    Image = 6,
    Utf8 = 7,
    File = 8,
    Voice = 9,

    #[num_enum(default)]
    Unknwown,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Descriptor {
    pub r#type: MessageType,
    pub header_len: u16,
    pub content_len: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ServerHeader<'u, 'f> {
    pub timestamp: DateTime<Utc>,
    pub from: &'u str,

    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<&'f str>,
}

impl<'u, 'f> Default for ServerHeader<'u, 'f> {
    fn default() -> Self {
        Self {
            timestamp: Utc::now(),
            from: "",
            filename: None,
        }
    }
}

impl<'u, 'f> ServerHeader<'u, 'f> {
    fn with_username(&mut self, uname: &'u str) -> &mut Self {
        self.from = uname;
        self
    }

    #[allow(dead_code)]
    fn with_filename(&mut self, filename: &'f str) -> &mut Self {
        self.filename = Some(filename);
        self
    }

    fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap()
    }
}

impl From<MessageType> for Descriptor {
    fn from(t: MessageType) -> Self {
        Self {
            r#type: t,
            header_len: 0,
            content_len: 0,
        }
    }
}

impl Descriptor {
    pub fn with_content_len(mut self, content_len: u64) -> Self {
        self.content_len = content_len;
        self
    }

    pub fn with_header_len(mut self, header_len: u16) -> Self {
        self.header_len = header_len;
        self
    }

    #[inline(always)]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert_eq!(bytes.len(), std::mem::size_of::<Self>());
        let r#type = MessageType::from((bytes[0] as u16) | ((bytes[1] as u16) << 8));
        let header_len = (bytes[2] as u16) | ((bytes[3] as u16) << 8);
        let mut content_len = 0u64;
        // SAFETY: this is safe because `content_len` is never unaligned and `src` and `dst` are treated as bytes
        unsafe {
            std::ptr::copy_nonoverlapping(
                // start from 8th byte, because of aligment
                bytes[8..].as_ptr(),
                &mut content_len as *mut u64 as *mut u8,
                std::mem::size_of_val(&content_len),
            );
        }
        Self {
            r#type,
            header_len,
            content_len,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: this is safe because byte slices do not need to be aligned
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }

    pub async fn read<R: AsyncReadExt>(mut reader: Pin<&mut R>) -> io::Result<Self> {
        let mut buf = [0u8; std::mem::size_of::<Self>()];
        reader.read_exact(&mut buf).await?;
        Ok(Self::from_bytes(&buf[..]))
    }
}
