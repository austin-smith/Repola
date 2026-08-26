use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("Could not read protocol frame: {0}")]
    Read(#[source] std::io::Error),
    #[error("Could not write protocol frame: {0}")]
    Write(#[source] std::io::Error),
    #[error("Protocol frame declared an invalid length of {length} bytes")]
    InvalidLength { length: usize },
    #[error("Protocol frame is {length} bytes; the limit is {maximum} bytes")]
    TooLarge { length: usize, maximum: usize },
    #[error("Could not decode protocol JSON: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("Could not encode protocol JSON: {0}")]
    Encode(#[source] serde_json::Error),
}

pub fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>, FrameError>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut length_bytes = [0_u8; 4];
    match reader.read(&mut length_bytes[..1]) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!("a one-byte buffer cannot read more than one byte"),
        Err(error) => return Err(FrameError::Read(error)),
    }
    reader
        .read_exact(&mut length_bytes[1..])
        .map_err(FrameError::Read)?;

    let length = u32::from_be_bytes(length_bytes) as usize;
    if length == 0 {
        return Err(FrameError::InvalidLength { length });
    }
    if length > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            length,
            maximum: MAX_FRAME_BYTES,
        });
    }

    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).map_err(FrameError::Read)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(FrameError::Decode)
}

pub fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), FrameError>
where
    W: Write,
    T: Serialize,
{
    let payload = serde_json::to_vec(value).map_err(FrameError::Encode)?;
    if payload.is_empty() {
        return Err(FrameError::InvalidLength { length: 0 });
    }
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            length: payload.len(),
            maximum: MAX_FRAME_BYTES,
        });
    }
    let length = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge {
        length: payload.len(),
        maximum: MAX_FRAME_BYTES,
    })?;
    writer
        .write_all(&length.to_be_bytes())
        .and_then(|()| writer.write_all(&payload))
        .and_then(|()| writer.flush())
        .map_err(FrameError::Write)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
    struct Example {
        value: String,
    }

    #[test]
    fn frames_round_trip_and_preserve_newlines() {
        let expected = Example {
            value: "line one\nline two\0still data".into(),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &expected).expect("write frame");

        let mut cursor = Cursor::new(bytes);
        let actual = read_frame(&mut cursor).expect("read frame");
        assert_eq!(actual, Some(expected));
        assert_eq!(read_frame::<_, Example>(&mut cursor).expect("eof"), None);
    }

    #[test]
    fn multiple_frames_can_share_a_stream() {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &Example {
                value: "one".into(),
            },
        )
        .expect("first");
        write_frame(
            &mut bytes,
            &Example {
                value: "two".into(),
            },
        )
        .expect("second");
        let mut cursor = Cursor::new(bytes);

        assert_eq!(
            read_frame(&mut cursor).expect("first read"),
            Some(Example {
                value: "one".into()
            })
        );
        assert_eq!(
            read_frame(&mut cursor).expect("second read"),
            Some(Example {
                value: "two".into()
            })
        );
    }

    #[test]
    fn oversized_frames_are_rejected_before_allocating_payload_memory() {
        let declared = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        let error = read_frame::<_, Example>(&mut Cursor::new(declared)).expect_err("too large");
        assert!(matches!(error, FrameError::TooLarge { .. }));
    }

    #[test]
    fn truncated_frames_are_rejected() {
        let mut bytes = 20_u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"short");
        let error = read_frame::<_, Example>(&mut Cursor::new(bytes)).expect_err("truncated");
        assert!(matches!(error, FrameError::Read(_)));
    }
}
