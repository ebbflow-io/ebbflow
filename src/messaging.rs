use std::collections::HashMap;

const VERSION: &'static str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub enum MessageError {
    Parse,
    UnknownMessage,
    Internal(&'static str),
}

#[derive(Debug, PartialEq)]
pub enum Message {
    /// Client -> Ebbflow - Hi, this connection is for this endpoint (TLS or SSH)
    HelloV0(HelloV0),
    StatusRequestV0,
    StatusResponseV0(StatusResponseV0),
    EnableDisableRequestV0(EnableDisableRequestV0),
    EnableDisableResponseV0,
    HelloResponseV0(HelloResponseV0),
    StartTrafficV0,
    StartTrafficResponseV0(StartTrafficResponseV0),
}

impl From<serde_cbor::Error> for MessageError {
    fn from(_e: serde_cbor::Error) -> Self {
        MessageError::Parse
    }
}

impl Message {
    /// length of message and payload u32 (4 + payload len)
    /// message id u32
    /// payload
    pub fn to_wire_message(&self) -> Result<Vec<u8>, MessageError> {
        let mut message_id = self.message_id().to_be_bytes().to_vec();
        let mut payload = self.payload()?;
        let len: u32 = message_id.len() as u32 + payload.len() as u32;

        let mut message = len.to_be_bytes().to_vec();
        message.append(&mut message_id);
        message.append(&mut payload);

        Ok(message)
    }

    /// give this the buffer WIHTOUT the length, e.g. message id then payload
    pub fn from_wire_without_the_length_prefix(buf: &[u8]) -> Result<Message, MessageError> {
        if buf.len() < 4 {
            return Err(MessageError::Parse);
        }

        let mut messagebuf: [u8; 4] = [0; 4];
        messagebuf.copy_from_slice(&buf[0..4]);

        let messageid: u32 = u32::from_be_bytes(messagebuf);

        let payload = &buf[4..];

        match messageid {
            1 => Ok(Message::HelloV0(serde_cbor::from_slice(&payload)?)),
            2 => Ok(Message::StatusRequestV0),
            3 => Ok(Message::StatusResponseV0(serde_cbor::from_slice(&payload)?)),
            4 => Ok(Message::EnableDisableRequestV0(serde_cbor::from_slice(
                &payload,
            )?)),
            5 => Ok(Message::EnableDisableResponseV0),
            6 => Ok(Message::HelloResponseV0(serde_cbor::from_slice(&payload)?)),
            7 => Ok(Message::StartTrafficV0),
            8 => Ok(Message::StartTrafficResponseV0(serde_cbor::from_slice(
                &payload,
            )?)),
            _ => Err(MessageError::UnknownMessage),
        }
    }

    fn payload(&self) -> Result<Vec<u8>, MessageError> {
        use Message::*;
        Ok(match self {
            HelloV0(x) => serde_cbor::to_vec(&x)?,
            StatusRequestV0 => vec![],
            StatusResponseV0(x) => serde_cbor::to_vec(&x)?,
            EnableDisableRequestV0(x) => serde_cbor::to_vec(&x)?,
            EnableDisableResponseV0 => vec![],
            HelloResponseV0(x) => serde_cbor::to_vec(&x)?,
            StartTrafficV0 => vec![],
            StartTrafficResponseV0(x) => serde_cbor::to_vec(&x)?,
        })
    }

    fn message_id(&self) -> u32 {
        use Message::*;
        match self {
            HelloV0(_) => 1,
            StatusRequestV0 => 2,
            StatusResponseV0(_) => 3,
            EnableDisableRequestV0(_) => 4,
            EnableDisableResponseV0 => 5,
            HelloResponseV0(_) => 6,
            StartTrafficV0 => 7,
            StartTrafficResponseV0(_) => 8,
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum EndpointType {
    Ssh,
    Tls,
}

/// This message is a ServerHello v0.
#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct HelloV0 {
    /// The key
    pub key: String,
    /// The type of the endpoint for this connection
    pub endpoint_type: EndpointType,
    /// e.g. test.mywebsite.com if Tls, or my-hostname if Ssh
    pub endpoint_value: String,
    /// random KV pairs
    pub meta: HashMap<String, String>,
}

impl HelloV0 {
    pub fn new(key: String, endpoint: EndpointType, e: String) -> HelloV0 {
        HelloV0 {
            key,
            endpoint_type: endpoint,
            endpoint_value: e,
            // Important: Never add customer-identifying data or here, privacy is extremely important
            meta: {
                let mut map = HashMap::new();
                map.insert("version".to_string(), VERSION.to_string());
                map
            },
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum EnableDisableTarget {
    AllEverything,
    AllEndpoints,
    Ssh,
    SpecificEndpoints(Vec<String>),
}

/// `should_be_running` dictates what this message should do, and then the rest of the args
/// specify which endpoints are targeted by the request.
#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct EnableDisableRequestV0 {
    pub should_be_running: bool,
    pub target: EnableDisableTarget,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct StatusResponseV0 {
    pub statuses: Vec<StatusV0>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct StatusV0 {
    /// ssh or tls
    pub endpoint_type: EndpointType,
    /// e.g. myhost or test.mysite.com
    pub endpoint_value: String,
    /// is this bad boi enabled?
    pub enabled: bool,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum HelloResponseIssue {
    NotFound,
    Forbidden,
    BadRequest,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct HelloResponseV0 {
    pub issue: Option<HelloResponseIssue>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct StartTrafficResponseV0 {
    pub open_local_success_ready: bool,
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn to_from_wire() {
        let key = "testkey";
        let etype = EndpointType::Ssh;
        let endpoint = "myhostname";
        let meta = HashMap::new();

        let hv0 = HelloV0 {
            key: key.to_string(),
            endpoint_type: etype,
            endpoint_value: endpoint.to_string(),
            meta,
        };

        let message = Message::HelloV0(hv0);
        let wire_message = message.to_wire_message().unwrap();

        let mut lenbuf = [0; 4];
        lenbuf.copy_from_slice(&wire_message[0..4]);
        let len: u32 = u32::from_be_bytes(lenbuf);

        // Verify the length we parsed from the wire_message is equal to the wire_message's buf len minus 4.
        assert_eq!(wire_message.len() - 4, len as usize);

        let parsed_from_wire =
            Message::from_wire_without_the_length_prefix(&wire_message[4..]).unwrap();

        // The two messages should be equal!
        assert_eq!(message, parsed_from_wire);
    }
}
