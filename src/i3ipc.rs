use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{env, error::Error, fmt, io, io::prelude::*, os::unix::net::UnixStream, process};

// Copy heavily from https://github.com/tmerr/i3ipc-rs

#[derive(Debug)]
pub enum MessageError {
    /// Network error sending the message.
    Send(io::Error),
    /// Network error receiving the response.
    Receive(io::Error),
    /// Got the response but couldn't parse the JSON.
    JsonCouldntParse(serde_json::Error),
}

impl Error for MessageError {
    fn cause(&self) -> Option<&dyn Error> {
        match *self {
            MessageError::Send(ref e) | MessageError::Receive(ref e) => Some(e),
            MessageError::JsonCouldntParse(ref e) => Some(e),
        }
    }
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match *self {
                MessageError::Send(_) => "Network error while sending message to i3",
                MessageError::Receive(_) => "Network error while receiving message from i3",
                MessageError::JsonCouldntParse(_) => {
                    "Got a response from i3 but couldn't parse the JSON"
                }
            }
        )
    }
}

trait I3Funcs {
    fn send_i3_message(&mut self, message_type: u32, payload: &str) -> io::Result<()>;
    fn receive_i3_message(&mut self) -> io::Result<(u32, String)>;
    fn send_receive_i3_message<T: serde::de::DeserializeOwned>(
        &mut self,
        message_type: u32,
        payload: &str,
    ) -> Result<T, MessageError>;
}

impl I3Funcs for UnixStream {
    fn send_i3_message(&mut self, message_type: u32, payload: &str) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(14 + payload.len());
        bytes.extend("i3-ipc".bytes()); // 6 bytes
        bytes.write_u32::<LittleEndian>(payload.len() as u32)?; // 4 bytes
        bytes.write_u32::<LittleEndian>(message_type)?; // 4 bytes
        bytes.extend(payload.bytes()); // payload.len() bytes
        self.write_all(&bytes[..])
    }

    /// returns a tuple of (message type, payload)
    fn receive_i3_message(&mut self) -> io::Result<(u32, String)> {
        let mut magic_data = [0_u8; 6];
        self.read_exact(&mut magic_data)?;
        let magic_string = String::from_utf8_lossy(&magic_data);
        if magic_string != "i3-ipc" {
            let error_text = format!(
                "unexpected magic string: expected 'i3-ipc' but got {}",
                magic_string
            );
            return Err(io::Error::new(io::ErrorKind::Other, error_text));
        }
        let payload_len = self.read_u32::<LittleEndian>()?;
        let message_type = self.read_u32::<LittleEndian>()?;
        let mut payload_data = vec![0_u8; payload_len as usize];
        self.read_exact(&mut payload_data[..])?;
        let payload_string = String::from_utf8_lossy(&payload_data).into_owned();
        Ok((message_type, payload_string))
    }

    fn send_receive_i3_message<T: serde::de::DeserializeOwned>(
        &mut self,
        message_type: u32,
        payload: &str,
    ) -> Result<T, MessageError> {
        if let Err(e) = self.send_i3_message(message_type, payload) {
            return Err(MessageError::Send(e));
        }
        let received = match self.receive_i3_message() {
            Ok((received_type, payload)) => {
                assert_eq!(message_type, received_type);
                payload
            }
            Err(e) => {
                return Err(MessageError::Receive(e));
            }
        };
        match serde_json::from_str(&received) {
            Ok(v) => Ok(v),
            Err(e) => Err(MessageError::JsonCouldntParse(e)),
        }
    }
}

/// The outcome of a single command.
#[derive(Debug)]
pub struct CommandOutcome {
    /// Whether the command was successful.
    pub success: bool,
    /// A human-readable error message.
    pub error: Option<String>,
}

/// The reply to the `command` request.
#[derive(Debug)]
pub struct Command {
    /// A list of `CommandOutcome` structs; one for each command that was parsed.
    pub outcomes: Vec<CommandOutcome>,
}

fn get_socket_path() -> io::Result<String> {
    if let Ok(sockpath) = env::var("I3SOCK") {
        return Ok(sockpath);
    }

    let output = process::Command::new("i3")
        .arg("--get-socketpath")
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end_matches('\n')
            .to_owned())
    } else {
        let prefix = "i3 --get-socketpath didn't return 0";
        let error_text = if !output.stderr.is_empty() {
            format!("{}. stderr: {:?}", prefix, output.stderr)
        } else {
            prefix.to_owned()
        };
        let error = io::Error::new(io::ErrorKind::Other, error_text);
        Err(error)
    }
}

pub fn connect_i3() -> Result<UnixStream, io::Error> {
    match get_socket_path() {
        Ok(path) => match UnixStream::connect(path) {
            Ok(stream) => Ok(stream),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}

pub fn run_command(stream: &mut UnixStream, string: &str) -> Result<Command, MessageError> {
    let j: serde_json::Value = stream.send_receive_i3_message(0, string)?;
    let commands = j.as_array().unwrap();
    let vec: Vec<_> = commands
        .iter()
        .map(|c| CommandOutcome {
            success: c.get("success").unwrap().as_bool().unwrap(),
            error: match c.get("error") {
                Some(val) => Some(val.as_str().unwrap().to_owned()),
                None => c.get("parse_error").map(|_| "Parse error".to_owned()),
            },
        })
        .collect();

    Ok(Command { outcomes: vec })
}

pub fn get_tree(stream: &mut UnixStream) -> Result<serde_json::Value, MessageError> {
    let val: serde_json::Value = stream.send_receive_i3_message(4, "")?;
    Ok(val)
}

pub fn get_workspaces(stream: &mut UnixStream) -> Result<serde_json::Value, MessageError> {
    let val: serde_json::Value = stream.send_receive_i3_message(1, "")?;
    Ok(val)
}
