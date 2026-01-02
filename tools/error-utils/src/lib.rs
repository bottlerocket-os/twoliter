#![allow(clippy::disallowed_types)]

use aws_smithy_types::error::display::DisplayErrorContext;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use std::error::Error as StdError;
use std::fmt;

/// Shadow the AWS SDK error type.
pub type SdkError<E, R = ::aws_smithy_runtime_api::client::orchestrator::HttpResponse> =
    ::aws_smithy_runtime_api::client::result::SdkError<E, R>;

/// Wrapper around AWS SDK errors that ensures consistent formatting
pub struct AwsSdkError<E, R = ::aws_smithy_runtime_api::client::orchestrator::HttpResponse>(
    pub SdkError<E, R>,
)
where
    E: ProvideErrorMetadata + StdError + 'static;

impl<E> fmt::Display for AwsSdkError<E>
where
    E: ProvideErrorMetadata + StdError + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_aws_sdk_error(&self.0))
    }
}

impl<E> fmt::Debug for AwsSdkError<E>
where
    E: ProvideErrorMetadata + StdError + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<E> StdError for AwsSdkError<E>
where
    E: ProvideErrorMetadata + StdError + 'static,
{
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.0)
    }
}

impl<E> From<SdkError<E>> for AwsSdkError<E>
where
    E: ProvideErrorMetadata + StdError + 'static,
{
    fn from(error: SdkError<E>) -> Self {
        AwsSdkError(error)
    }
}

impl<E> AwsSdkError<E>
where
    E: ProvideErrorMetadata + StdError + 'static,
{
    /// Get the service error from the SDK error, if available
    pub fn as_service_error(&self) -> Option<&E> {
        self.0.as_service_error()
    }
}

/// Helper function to format AWS SDK errors as "code: message" or fall back to full context
///
/// # Examples
///
/// With code only: "AccessDenied"
/// With message only: "Request timeout occurred"
/// With neither: Falls back to DisplayErrorContext for full error details
pub fn format_aws_sdk_error<E>(sdk_error: &SdkError<E>) -> String
where
    E: ProvideErrorMetadata + StdError + 'static,
{
    match (sdk_error.code(), sdk_error.message()) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => format!("{}", DisplayErrorContext(sdk_error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_ssm::operation::get_parameters::GetParametersError;
    use aws_smithy_types::error::metadata::ErrorMetadata;

    #[test]
    fn test_error_metadata_extraction() {
        // Test that our logic works with error metadata
        let error_metadata = ErrorMetadata::builder()
            .code("InvalidParameter")
            .message("Parameter is invalid")
            .build();

        let service_error = GetParametersError::generic(error_metadata);

        // Verify the error has the expected code and message
        assert_eq!(service_error.code(), Some("InvalidParameter"));
        assert_eq!(service_error.message(), Some("Parameter is invalid"));
    }

    #[test]
    fn test_aws_sdk_error_wrapper_display() {
        let sdk_error: SdkError<GetParametersError> = SdkError::timeout_error("request timeout");
        let wrapper = AwsSdkError(sdk_error);

        // The wrapper should use our format_aws_sdk_error function
        let result = wrapper.to_string();
        assert_eq!(result, "request has timed out: request timeout (TimeoutError(TimeoutError { source: \"request timeout\" }))");
    }
}
