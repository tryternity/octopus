use crate::{config::VisionBackend, error::PaddleOcrError, error::Result};

pub const OPENCV_BACKEND_DISABLED_MESSAGE: &str =
    "OpenCV backend requested but crate feature `opencv-backend` is not enabled";

#[cfg(test)]
pub fn default_backend() -> VisionBackend {
    VisionBackend::default()
}

pub fn resolve_backend_strict(backend: VisionBackend) -> Result<VisionBackend> {
    match backend {
        VisionBackend::PureRust => Ok(VisionBackend::PureRust),
        VisionBackend::OpenCv => Err(PaddleOcrError::Config(
            OPENCV_BACKEND_DISABLED_MESSAGE.to_string(),
        )),
    }
}

pub fn resolve_backend_or_pure_rust(backend: VisionBackend) -> VisionBackend {
    resolve_backend_strict(backend).unwrap_or(VisionBackend::PureRust)
}

#[cfg(test)]
mod tests {
    use super::{OPENCV_BACKEND_DISABLED_MESSAGE, resolve_backend_or_pure_rust, resolve_backend_strict};
    use crate::config::VisionBackend;

    #[test]
    fn pure_rust_backend_is_always_supported() {
        let resolved =
            resolve_backend_strict(VisionBackend::PureRust).expect("pure rust should be supported");
        assert_eq!(resolved, VisionBackend::PureRust);
    }

    #[test]
    fn fallback_policy_can_downgrade_to_pure_rust() {
        let resolved = resolve_backend_or_pure_rust(VisionBackend::OpenCv);
        assert_eq!(resolved, VisionBackend::PureRust);
    }

    #[test]
    fn strict_policy_rejects_unsupported_backends() {
        let err = resolve_backend_strict(VisionBackend::OpenCv)
            .expect_err("strict policy should reject unsupported backend");
        assert!(err.to_string().contains(OPENCV_BACKEND_DISABLED_MESSAGE));
    }
}
