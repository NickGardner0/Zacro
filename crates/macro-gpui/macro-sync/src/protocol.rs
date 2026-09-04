use thiserror::Error;

const FRAME_PREFIX_LEN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerMessage {
    Update { updates: Vec<Vec<u8>>, id: String },
    Awareness { awareness: Vec<u8> },
    RequestSince { version_vector: Vec<u8> },
    RequestSnapshot,
    RegisterPeerId { peer_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteMessage {
    InitialSync {
        snapshot: Vec<u8>,
        awareness: Vec<u8>,
    },
    Update {
        update: Vec<u8>,
    },
    Awareness {
        awareness: Vec<u8>,
    },
    Snapshot {
        snapshot: Vec<u8>,
    },
    UpdateAck {
        id: String,
    },
    UpdateSince {
        update: Vec<u8>,
        version_vector: Vec<u8>,
    },
    /// Forward-compatible representation of a union discriminator introduced
    /// by a newer sync service.
    Unknown {
        discriminator: u8,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("Bebop frame is truncated at byte {offset}")]
    Truncated { offset: usize },
    #[error("Bebop frame declares {declared} bytes but contains {actual}")]
    InvalidFrameLength { declared: usize, actual: usize },
    #[error("Bebop length exceeds the protocol's u32 limit")]
    LengthOverflow,
    #[error("Bebop string is not valid UTF-8")]
    InvalidUtf8,
    #[error("Bebop peer frame has unknown union discriminator {discriminator}")]
    UnknownPeerDiscriminator { discriminator: u8 },
}

impl PeerMessage {
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let (discriminator, payload) = match self {
            Self::Update { updates, id } => {
                let mut payload = Vec::new();
                write_len(&mut payload, updates.len())?;
                for update in updates {
                    write_bytes(&mut payload, update)?;
                }
                write_string(&mut payload, id)?;
                (1, payload)
            }
            Self::Awareness { awareness } => {
                let mut payload = Vec::new();
                write_bytes(&mut payload, awareness)?;
                (2, payload)
            }
            Self::RequestSince { version_vector } => {
                let mut payload = Vec::new();
                write_bytes(&mut payload, version_vector)?;
                (3, payload)
            }
            Self::RequestSnapshot => (4, Vec::new()),
            Self::RegisterPeerId { peer_id } => (5, peer_id.to_le_bytes().to_vec()),
        };
        encode_union(discriminator, payload)
    }

    /// Decode an outbound Macro sync-service frame. Besides providing a
    /// symmetric protocol implementation, this is used by cold-start recovery
    /// to replay the exact Loro updates stored in the durable WAL without
    /// changing their idempotency key or wire representation.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (discriminator, payload) = decode_union(bytes)?;
        let mut reader = Reader::new(payload);
        let message = match discriminator {
            1 => {
                let count = reader.read_u32()? as usize;
                let mut updates = Vec::with_capacity(count);
                for _ in 0..count {
                    updates.push(reader.read_bytes()?.to_vec());
                }
                Self::Update {
                    updates,
                    id: reader.read_string()?.to_owned(),
                }
            }
            2 => Self::Awareness {
                awareness: reader.read_bytes()?.to_vec(),
            },
            3 => Self::RequestSince {
                version_vector: reader.read_bytes()?.to_vec(),
            },
            4 => Self::RequestSnapshot,
            5 => Self::RegisterPeerId {
                peer_id: reader.read_u64()?,
            },
            discriminator => {
                return Err(ProtocolError::UnknownPeerDiscriminator { discriminator });
            }
        };
        reader.finish()?;
        Ok(message)
    }
}

impl RemoteMessage {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (discriminator, payload) = decode_union(bytes)?;
        let mut reader = Reader::new(payload);
        let message = match discriminator {
            1 => Self::InitialSync {
                snapshot: reader.read_bytes()?.to_vec(),
                awareness: reader.read_bytes()?.to_vec(),
            },
            2 => Self::Update {
                update: reader.read_bytes()?.to_vec(),
            },
            3 => Self::Awareness {
                awareness: reader.read_bytes()?.to_vec(),
            },
            4 => Self::Snapshot {
                snapshot: reader.read_bytes()?.to_vec(),
            },
            5 => Self::UpdateAck {
                id: reader.read_string()?.to_owned(),
            },
            6 => Self::UpdateSince {
                update: reader.read_bytes()?.to_vec(),
                version_vector: reader.read_bytes()?.to_vec(),
            },
            discriminator => {
                return Ok(Self::Unknown {
                    discriminator,
                    payload: payload.to_vec(),
                });
            }
        };
        reader.finish()?;
        Ok(message)
    }

    #[cfg(test)]
    pub(crate) fn encode_for_fixture(&self) -> Result<Vec<u8>, ProtocolError> {
        let (discriminator, payload) = match self {
            Self::InitialSync {
                snapshot,
                awareness,
            } => {
                let mut payload = Vec::new();
                write_bytes(&mut payload, snapshot)?;
                write_bytes(&mut payload, awareness)?;
                (1, payload)
            }
            Self::Update { update } => {
                let mut payload = Vec::new();
                write_bytes(&mut payload, update)?;
                (2, payload)
            }
            Self::Awareness { awareness } => {
                let mut payload = Vec::new();
                write_bytes(&mut payload, awareness)?;
                (3, payload)
            }
            Self::Snapshot { snapshot } => {
                let mut payload = Vec::new();
                write_bytes(&mut payload, snapshot)?;
                (4, payload)
            }
            Self::UpdateAck { id } => {
                let mut payload = Vec::new();
                write_string(&mut payload, id)?;
                (5, payload)
            }
            Self::UpdateSince {
                update,
                version_vector,
            } => {
                let mut payload = Vec::new();
                write_bytes(&mut payload, update)?;
                write_bytes(&mut payload, version_vector)?;
                (6, payload)
            }
            Self::Unknown {
                discriminator,
                payload,
            } => (*discriminator, payload.clone()),
        };
        encode_union(discriminator, payload)
    }
}

fn encode_union(discriminator: u8, payload: Vec<u8>) -> Result<Vec<u8>, ProtocolError> {
    let mut frame = Vec::with_capacity(FRAME_PREFIX_LEN + payload.len());
    write_len(&mut frame, payload.len())?;
    frame.push(discriminator);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn decode_union(bytes: &[u8]) -> Result<(u8, &[u8]), ProtocolError> {
    let mut reader = Reader::new(bytes);
    let payload_len = reader.read_u32()? as usize;
    let discriminator = reader.read_u8()?;
    let declared = FRAME_PREFIX_LEN
        .checked_add(payload_len)
        .ok_or(ProtocolError::LengthOverflow)?;
    if declared != bytes.len() {
        return Err(ProtocolError::InvalidFrameLength {
            declared,
            actual: bytes.len(),
        });
    }
    Ok((discriminator, &bytes[FRAME_PREFIX_LEN..]))
}

fn write_len(destination: &mut Vec<u8>, len: usize) -> Result<(), ProtocolError> {
    let len = u32::try_from(len).map_err(|_| ProtocolError::LengthOverflow)?;
    destination.extend_from_slice(&len.to_le_bytes());
    Ok(())
}

fn write_bytes(destination: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ProtocolError> {
    write_len(destination, bytes.len())?;
    destination.extend_from_slice(bytes);
    Ok(())
}

fn write_string(destination: &mut Vec<u8>, value: &str) -> Result<(), ProtocolError> {
    write_bytes(destination, value.as_bytes())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        let value = *self
            .bytes
            .get(self.offset)
            .ok_or(ProtocolError::Truncated {
                offset: self.offset,
            })?;
        self.offset += 1;
        Ok(value)
    }

    fn read_u32(&mut self) -> Result<u32, ProtocolError> {
        let end = self
            .offset
            .checked_add(4)
            .ok_or(ProtocolError::LengthOverflow)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ProtocolError::Truncated {
                offset: self.offset,
            })?;
        self.offset = end;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64, ProtocolError> {
        let end = self
            .offset
            .checked_add(8)
            .ok_or(ProtocolError::LengthOverflow)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ProtocolError::Truncated {
                offset: self.offset,
            })?;
        self.offset = end;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    fn read_bytes(&mut self) -> Result<&'a [u8], ProtocolError> {
        let len = self.read_u32()? as usize;
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ProtocolError::LengthOverflow)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ProtocolError::Truncated {
                offset: self.offset,
            })?;
        self.offset = end;
        Ok(bytes)
    }

    fn read_string(&mut self) -> Result<&'a str, ProtocolError> {
        std::str::from_utf8(self.read_bytes()?).map_err(|_| ProtocolError::InvalidUtf8)
    }

    fn finish(&self) -> Result<(), ProtocolError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(ProtocolError::InvalidFrameLength {
                declared: self.offset,
                actual: self.bytes.len(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const TYPESCRIPT_BEBOP_FIXTURES: &str = include_str!("../fixtures/sync_bebop_typescript.json");

    fn typescript_fixture_frames() -> HashMap<String, Vec<u8>> {
        let value: serde_json::Value = serde_json::from_str(TYPESCRIPT_BEBOP_FIXTURES).unwrap();
        assert_eq!(value["schema"], "macro.sync-service.bebop.typescript.v1");
        value["frames"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| {
                let hex = value.as_str().unwrap();
                assert_eq!(hex.len() % 2, 0, "fixture {name} has invalid hex");
                let bytes = (0..hex.len())
                    .step_by(2)
                    .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
                    .collect();
                (name.clone(), bytes)
            })
            .collect()
    }

    #[test]
    fn peer_update_matches_generated_bebop_layout() {
        let encoded = PeerMessage::Update {
            updates: vec![vec![0xaa, 0xbb], vec![0xcc]],
            id: "op-1".into(),
        }
        .encode()
        .unwrap();

        assert_eq!(
            encoded,
            vec![
                23, 0, 0, 0, // union payload length (excluding discriminator)
                1, // PeerUpdate
                2, 0, 0, 0, // updates count
                2, 0, 0, 0, 0xaa, 0xbb, // first byte[]
                1, 0, 0, 0, 0xcc, // second byte[]
                4, 0, 0, 0, b'o', b'p', b'-', b'1', // id
            ]
        );
    }

    #[test]
    fn rust_peer_frames_match_macro_typescript_generated_fixtures() {
        let fixtures = typescript_fixture_frames();
        let messages = [
            (
                "peer_update",
                PeerMessage::Update {
                    updates: vec![vec![0xaa, 0xbb], vec![0xcc]],
                    id: "op-1".into(),
                },
            ),
            (
                "peer_awareness",
                PeerMessage::Awareness {
                    awareness: vec![1, 2, 3],
                },
            ),
            (
                "peer_request_since",
                PeerMessage::RequestSince {
                    version_vector: vec![4, 5],
                },
            ),
            ("peer_request_snapshot", PeerMessage::RequestSnapshot),
            (
                "peer_register_id",
                PeerMessage::RegisterPeerId { peer_id: 42 },
            ),
        ];
        for (name, message) in messages {
            let encoded = message.encode().unwrap();
            assert_eq!(encoded, fixtures[name], "fixture {name}");
            assert_eq!(PeerMessage::decode(&encoded).unwrap(), message);
        }
    }

    #[test]
    fn rust_decodes_every_macro_typescript_remote_fixture() {
        let fixtures = typescript_fixture_frames();
        let messages = [
            (
                "remote_initial_sync",
                RemoteMessage::InitialSync {
                    snapshot: vec![1, 2],
                    awareness: vec![3],
                },
            ),
            ("remote_update", RemoteMessage::Update { update: vec![4] }),
            (
                "remote_awareness",
                RemoteMessage::Awareness { awareness: vec![5] },
            ),
            (
                "remote_snapshot",
                RemoteMessage::Snapshot {
                    snapshot: vec![6, 7],
                },
            ),
            (
                "remote_update_ack",
                RemoteMessage::UpdateAck { id: "abc".into() },
            ),
            (
                "remote_update_since",
                RemoteMessage::UpdateSince {
                    update: vec![8],
                    version_vector: vec![9, 10],
                },
            ),
        ];
        for (name, expected) in messages {
            assert_eq!(
                RemoteMessage::decode(&fixtures[name]).unwrap(),
                expected,
                "fixture {name}"
            );
        }
    }

    #[test]
    fn all_remote_variants_round_trip() {
        let fixtures = [
            RemoteMessage::InitialSync {
                snapshot: vec![1, 2],
                awareness: vec![3],
            },
            RemoteMessage::Update { update: vec![4] },
            RemoteMessage::Awareness { awareness: vec![5] },
            RemoteMessage::Snapshot {
                snapshot: vec![6, 7],
            },
            RemoteMessage::UpdateAck { id: "abc".into() },
            RemoteMessage::UpdateSince {
                update: vec![8],
                version_vector: vec![9, 10],
            },
            RemoteMessage::Unknown {
                discriminator: 42,
                payload: vec![11, 12],
            },
        ];

        for fixture in fixtures {
            let bytes = fixture.encode_for_fixture().unwrap();
            assert_eq!(RemoteMessage::decode(&bytes).unwrap(), fixture);
        }
    }

    #[test]
    fn rejects_truncated_and_trailing_frames() {
        assert!(matches!(
            RemoteMessage::decode(&[1, 0, 0]),
            Err(ProtocolError::Truncated { .. })
        ));
        assert!(matches!(
            RemoteMessage::decode(&[0, 0, 0, 0, 4, 99]),
            Err(ProtocolError::InvalidFrameLength { .. })
        ));
        assert!(matches!(
            PeerMessage::decode(&[0, 0, 0, 0, 99]),
            Err(ProtocolError::UnknownPeerDiscriminator { discriminator: 99 })
        ));
    }
}
