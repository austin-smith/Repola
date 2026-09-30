use std::ffi::OsString;
use std::path::Path;

use base64::prelude::{Engine as _, BASE64_STANDARD};

use super::command;
use super::models::{GitPath, ImageComparison, ImagePreview, ImageVersion};
use super::working_copy::path_from_token;

pub(super) const MAX_IMAGE_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;

pub(super) fn image_comparison(
    before: ImageVersion,
    after: ImageVersion,
) -> Option<ImageComparison> {
    if matches!(before, ImageVersion::Preview(_)) || matches!(after, ImageVersion::Preview(_)) {
        Some(ImageComparison { before, after })
    } else {
        None
    }
}

pub(super) fn image_preview(bytes: &[u8], label: &str) -> ImageVersion {
    if bytes.len() as u64 > MAX_IMAGE_PREVIEW_BYTES {
        return ImageVersion::TooLarge;
    }
    let mime_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else {
        return ImageVersion::Unsupported;
    };
    ImageVersion::Preview(ImagePreview {
        mime_type: mime_type.into(),
        base64: BASE64_STANDARD.encode(bytes),
        byte_length: bytes.len() as u64,
        label: label.into(),
    })
}

pub(super) fn revision_image_preview(
    worktree: &Path,
    revision: &str,
    path: &GitPath,
    label: &str,
) -> Result<ImageVersion, String> {
    let mut object = OsString::from(revision);
    object.push(":");
    object.push(path_from_token(&path.token)?);
    let size = command::git_at(
        worktree,
        [
            OsString::from("cat-file"),
            OsString::from("-s"),
            object.clone(),
        ],
    )
    .map_err(|error| error.to_string())?;
    if !size.status.success() {
        return Ok(ImageVersion::Missing);
    }
    let size = String::from_utf8_lossy(&size.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|_| "Git returned an invalid image blob size.".to_string())?;
    if size > MAX_IMAGE_PREVIEW_BYTES {
        return Ok(ImageVersion::TooLarge);
    }
    let blob = command::git_at(
        worktree,
        [OsString::from("cat-file"), OsString::from("blob"), object],
    )
    .map_err(|error| error.to_string())?;
    if !blob.status.success() {
        return Ok(ImageVersion::Missing);
    }
    Ok(image_preview(&blob.stdout, label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_preserves_the_exact_image_bytes_and_checks_signatures() {
        for (bytes, mime_type) in [
            (b"\x89PNG\r\n\x1a\n\0png".as_slice(), "image/png"),
            (b"\xff\xd8\xff\0jpeg", "image/jpeg"),
            (b"GIF87a\0gif", "image/gif"),
            (b"GIF89a\0gif", "image/gif"),
            (b"RIFF\0\0\0\0WEBP", "image/webp"),
            (b"BM\0bmp", "image/bmp"),
        ] {
            let ImageVersion::Preview(preview) = image_preview(bytes, "Before") else {
                panic!("expected image preview")
            };
            assert_eq!(preview.mime_type, mime_type);
            assert_eq!(preview.label, "Before");
            assert_eq!(preview.byte_length, bytes.len() as u64);
            assert_eq!(BASE64_STANDARD.decode(preview.base64).unwrap(), bytes);
        }
        assert!(matches!(
            image_preview(b"binary\0data", "Before"),
            ImageVersion::Unsupported
        ));
        assert!(matches!(
            image_preview(b"RIFF\0\0\0\0WAVE", "Before"),
            ImageVersion::Unsupported
        ));
    }

    #[test]
    fn each_side_is_bounded_and_serializes_with_explicit_availability() {
        let mut bytes = vec![0; MAX_IMAGE_PREVIEW_BYTES as usize];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        let comparison = image_comparison(
            image_preview(&bytes, "Before"),
            image_preview(&bytes, "After"),
        )
        .unwrap();
        let serialized = serde_json::to_vec(&comparison).unwrap();
        assert!(serialized.len() < crate::protocol::MAX_FRAME_BYTES);
        bytes.push(0);
        assert!(matches!(
            image_preview(&bytes, "After"),
            ImageVersion::TooLarge
        ));
        assert_eq!(
            serde_json::to_value(ImageVersion::Missing).unwrap(),
            serde_json::json!({"kind": "missing"})
        );
        assert_eq!(
            serde_json::to_value(ImageVersion::TooLarge).unwrap(),
            serde_json::json!({"kind": "tooLarge"})
        );
        assert!(image_comparison(ImageVersion::Missing, ImageVersion::Unsupported).is_none());
    }
}
