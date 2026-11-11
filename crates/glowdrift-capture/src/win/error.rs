//! HRESULT classification for capture failures.

use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED,
    DXGI_ERROR_DEVICE_RESET, DXGI_ERROR_SESSION_DISCONNECTED, DXGI_ERROR_UNSUPPORTED,
    DXGI_ERROR_WAIT_TIMEOUT,
};

use crate::CaptureError;

/// Coarse meaning of an HRESULT returned while capturing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Timeout,
    AccessLost,
    Unsupported,
    Other,
}

pub(crate) fn classify(code: i32) -> Class {
    if code == DXGI_ERROR_WAIT_TIMEOUT.0 {
        Class::Timeout
    } else if [
        DXGI_ERROR_ACCESS_LOST.0,
        DXGI_ERROR_DEVICE_REMOVED.0,
        DXGI_ERROR_DEVICE_RESET.0,
        DXGI_ERROR_DEVICE_HUNG.0,
        DXGI_ERROR_SESSION_DISCONNECTED.0,
        // Returned by DuplicateOutput while the secure desktop is active.
        E_ACCESSDENIED.0,
    ]
    .contains(&code)
    {
        Class::AccessLost
    } else if code == DXGI_ERROR_UNSUPPORTED.0 {
        Class::Unsupported
    } else {
        Class::Other
    }
}

pub(crate) fn map_err(context: &str, error: &windows::core::Error) -> CaptureError {
    match classify(error.code().0) {
        Class::AccessLost => CaptureError::AccessLost,
        Class::Unsupported => CaptureError::Unsupported,
        Class::Timeout | Class::Other => CaptureError::Os(format!("{context}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_codes() {
        assert_eq!(classify(DXGI_ERROR_WAIT_TIMEOUT.0), Class::Timeout);
        assert_eq!(classify(DXGI_ERROR_ACCESS_LOST.0), Class::AccessLost);
        assert_eq!(classify(DXGI_ERROR_DEVICE_REMOVED.0), Class::AccessLost);
        assert_eq!(classify(DXGI_ERROR_DEVICE_RESET.0), Class::AccessLost);
        assert_eq!(classify(E_ACCESSDENIED.0), Class::AccessLost);
        assert_eq!(classify(DXGI_ERROR_UNSUPPORTED.0), Class::Unsupported);
        assert_eq!(classify(windows::Win32::Foundation::E_FAIL.0), Class::Other);
        assert_eq!(classify(0), Class::Other);
    }

    #[test]
    fn maps_to_capture_errors() {
        let lost = windows::core::Error::from(DXGI_ERROR_ACCESS_LOST);
        assert!(matches!(map_err("x", &lost), CaptureError::AccessLost));
        let other = windows::core::Error::from(windows::Win32::Foundation::E_FAIL);
        assert!(matches!(map_err("x", &other), CaptureError::Os(_)));
    }
}
